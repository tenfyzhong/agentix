# Memory performance measurements

Local measurements on 2026-09-30, Apple Silicon macOS, Rust 1.95. Temporary SQLite
databases and local mock providers only. No live credentials or production data.
No matched pre-change benchmark was collected, so these are current measurements,
not percentage speedup claims.

## Reproduction

Run serially; concurrent builds or other host activity affect tail latency.

```sh
cargo test -p agentix-memory --test performance -- --ignored --nocapture --test-threads=1
cargo test --release -p agentix-memory --test performance ten_thousand -- --ignored --nocapture --test-threads=1
cargo test -p agentix-memory --test query_coalescing -- --ignored --nocapture
cargo test -p agentix-memory --lib query_admission_latency -- --ignored --nocapture
cargo test -p taskix idle_daemon_cpu -- --ignored --nocapture --test-threads=1
```

## Retrieval and SQLite

Exact cosine recall, 384 dimensions, top 8, 256-row pages. FTS and hybrid timings
include database reads, ranking and result materialization, but exclude CLI/IPC
and remote embedding latency. Retrieval runs alongside 100 source ingests.
Percentiles use nearest-rank selection from 30 samples; p99 is the sample maximum.

| Scenario | Build | p50 | p95 | p99 |
| --- | --- | ---: | ---: | ---: |
| 2,000 target / 8,000 foreign memories, FTS | debug | 4.82 ms | 5.01 ms | 5.03 ms |
| Same dataset, hybrid | debug | 23.42 ms | 24.67 ms | 27.44 ms |
| 10,000 target memories, FTS | debug | 17.93 ms | 19.74 ms | 73.19 ms |
| Same dataset, hybrid | debug | 119.78 ms | 150.30 ms | 373.69 ms |
| 10,000 target memories, FTS | release | 7.17 ms | 7.65 ms | 7.69 ms |
| Same dataset, hybrid | release | 44.62 ms | 50.65 ms | 50.74 ms |
| Context packet preparation, 10,000 target memories | release | 0.236 ms | 0.550 ms | 1.357 ms |
| Context receipt hit, same dataset | release | 0.115 ms | 0.159 ms | 0.494 ms |
| Missing context receipt while a writer is held | debug | 0.251 ms | 0.573 ms | 0.617 ms |
| Writer acquisition behind a deliberate 20 ms blocker | debug | 27.14 ms | 67.51 ms | 76.33 ms |

Context timings use already retrieved candidates or an existing receipt; they
are not full host-hook timings. Reopening the 10,000-memory database and doing
the first FTS query took 19.41 ms in release and 256.11 ms in debug. This is a
connection-cold measurement, not an OS-cache-cold or fresh-process tokenizer test.

## Concurrency and provider admission

With a 100 ms local HTTP delay, 16 simultaneous identical semantic queries made
**one provider request**. Debug p50 was 107.51 ms; p95/p99 were 369.95 ms. This is
a small harness result under local host load, not a network-service latency bound.

A separate admission benchmark holds three background permits on a provider
limited to four total requests. Across 100 online permit acquisitions, measured
p50/p95/p99 were 666/667/667 ns. This measures semaphore admission only, excluding
HTTP and model execution. Deterministic tests also verify that the total limit
is never exceeded and that single-capacity requests are not preempted.

## Idle scheduling

The debug scheduler harness registers 200 empty Projects and runs extraction
and embedding loops. After a 10-second warmup, a 5.003-second observation consumed
0.140 seconds of process CPU, approximately 2.80% of one core. CPU is sampled with
`ps` and includes test/runtime overhead. This excludes the source intake,
projection, IPC and full daemon process lifecycle; it is not whole-service idle
CPU acceptance. No provider calls are needed for empty Projects.

## Vector-index decision

Keep exact retrieval at the measured scale. Release hybrid p95 of 50.65 ms for
10,000 project vectors leaves room within the 750 ms semantic context deadline,
although embedding/network/queue time consumes the same budget. ANN is not added
without a larger-scale bottleneck and a measured recall requirement. This
decision does not establish an unlimited capacity guarantee: higher dimensions,
more memories, concurrent distinct scans and slower disks need fresh profiling.


## Atomic-memory audit (2026-10-06)

`initialized_open_and_ineligible_compaction_scale` measures 30 samples per size
in a debug build on the same local Apple Silicon host, with a warm OS page cache.
It bulk-loads valid human-memory JSON through the real insertion trigger, then
measures fresh writable opens and `next_compaction_at` calls. No model, IPC,
retained-source validation, FTS or vector work is included.

| Human memories | Open p50 / p95 | Dirty probe before p50 / p95 | Dirty probe after p50 / p95 |
| ---: | ---: | ---: | ---: |
| 1,000 | 1.066 / 1.299 ms | 2.272 / 2.326 ms | 0.479 / 0.548 ms |
| 10,000 | 1.089 / 1.171 ms | 18.594 / 21.973 ms | 0.481 / 0.524 ms |
| 100,000 | 1.137 / 1.330 ms | 192.726 / 203.327 ms | 0.507 / 0.604 ms |

Before/after probe measurements use the same fixture and harness in consecutive
runs. Human and retired revisions now have `dirty=0`; a partial candidate index
excludes suspended work. Expired pending candidates are consumed in bounded
pages without model calls. The deadline probe no longer joins memory JSON.
Outstanding dirty revisions waiting for old in-flight seeds still require work
state lookups; the table above does not measure a large waiting-work backlog.

Auxiliary-table backfills and trigger replacement now run once in an atomic
migration, not on every open. Sentinel-trigger tests reject any repeated backfill
attempt and check that reopening leaves SQLite's schema version unchanged.
Migration failure tests verify rollback, retry and preservation of existing
progress. The one-time migration still scans historical records; these open
measurements are after initialization and do not claim constant-time migration
or full-service startup.
