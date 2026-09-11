# Runtime and native bridge performance

The bridge refactor includes reusable component benchmarks and behavioral regression tests. These numbers describe this local run, not a provider throughput limit or a guarantee of IM latency.

## Reproduce

From the repository root:

```sh
node plugins/agentix-bridge/benchmarks/runtime.mjs
cargo bench -p agentix-bridge --bench list_sessions
node plugins/agentix-bridge/benchmarks/claude-state.mjs
```

The Node benchmark accepts `BENCH_SAMPLES`, `BENCH_HISTORY_ITERATIONS`, and `BENCH_QUEUE_ITEMS`. Defaults are three samples, 200 history queries per sample, and 500 queue deliveries. The Rust benchmark runs five measured samples after one warmup and builds in release mode. Neither invokes a model or sends real IM messages. The Rust list path includes the local best-effort rmux/process inventory lookup.

## Recorded results

Measured on September 9, 2026, on macOS arm64 with Node 26.8.1; the Rust listing benchmark used the release profile. Before/after samples use matching fixture sizes and benchmark procedures. The listing samples record platform and profile, but not the compiler version. Raw samples and captured environment fields are in [benchmarks/](benchmarks/).

| Workload | Before, median | After, median | Change |
| --- | ---: | ---: | --- |
| Page of 20 turns from 40,000 native entries | 1.289 ms | 0.00569 ms | Reuse the stable branch index |
| Enqueue, claim and finish 500 deliveries | 354.43 ms | 7.42 ms | Append queue mutations instead of full receipt snapshots |
| Serialized queue records for those deliveries | 51,604,235 bytes | 182,170 bytes | About 99.65% fewer serialized bytes |
| Extension `stop` while a 100 ms model lookup is pending | 97.77 ms | 6.52 ms | Stop bypasses unrelated command serialization |
| List 32 native connections, each with a 10 ms response delay | 508.93 ms | 86.92 ms | Up to 16 concurrent metadata requests |

Sources: [runtime before](benchmarks/runtime-before.json), [runtime after](benchmarks/runtime-after.json), [list before](benchmarks/list-sessions-before.json), [list after](benchmarks/list-sessions-after.json).

History samples include the first index construction in the first sample, followed by repeated queries of an unchanged branch. Real activity invalidates the index and incurs reconstruction. Hosts without a stable leaf API still rebuild on reads. The result therefore describes repeated history browsing, not arbitrary writes or branch switches.

Queue timing includes cloning and JSON serialization through the benchmark persistence callback. The byte count covers queue records, not all native session entries, turn text, association metadata, filesystem writes, or `fsync`. Receipts still grow with the number of distinct requests so replay can retain deduplication; this change reduces repeated writes rather than imposing a receipt eviction policy.

Stop measurement uses a local socket fixture with a five-millisecond polling interval. It verifies the extension scheduling improvement; service handlers, IM APIs, host abort behavior, and providers can add latency. Other extension mutations remain serialized.

The Rust fixture returns the same small snapshot-shaped JSON payload in both runs. The new adapter reads its metadata subset. Consequently, this comparison primarily captures concurrency and includes inventory/process overhead, scheduler variation, and transport cost. Production extensions also avoid constructing or transferring history and queue data during registration/listing. A stalled host still consumes its RPC deadline; bounded concurrency prevents unbounded fanout but is not a promise that a large list always returns immediately.

## Correctness constraints

Optimization retains FIFO ownership, session isolation, stable turn association, and explicit recovery. Tests verify linear journal growth, legacy checkpoint replay, duplicate record replay, uncertain in-flight recovery, failed append atomicity, history index invalidation, stop/read progress during pending commands, concurrent list requests, and rejection of foreign metadata. The full workspace checks and optional real Pi/OMP loader tests passed after the changes; see [integration coverage](integration-coverage.md).

Further optimization should start with a representative workload and a failing behavioral regression test when semantics change. These measurements do not establish a global performance maximum. Native storage cost, large active turn persistence, inventory lookup, and upstream command serialization remain workload-dependent costs.


## Claude and shared transport review

The [Claude state benchmark](../plugins/agentix-bridge/benchmarks/claude-state.mjs) compares full-snapshot serialization with the incremental persistence callback using the same session operations. It defaults to 500 turns and three samples; `BENCH_CLAUDE_TURNS` and `BENCH_SAMPLES` override them. Each turn has a short prompt and a 1,020-character reply.

On the same September 9 macOS arm64 / Node 26.8.1 environment, median processing time was 435.52 ms with snapshots and 3.45 ms with the journal callback. Serialized state fell from 475,248,986 to 1,366,617 bytes (about 99.71% less). [Raw samples](benchmarks/claude-state.json) include both paths. This measures session bookkeeping and JSON serialization, excluding filesystem writes, fsync, native transcript parsing, MCP model context, rmux, and IM latency. The store tests separately verify filesystem replay, legacy checkpoints, partial-tail repair, receipt deduplication, and linear serialized growth.

Claude now watches its mailbox rather than reading identity and scanning the directory ten times per idle second; a one-second fallback scan remains. Processing removes events only after success. Default rmux mode no longer advertises unused Channel tools/instructions. Shared Node framing assembles completed frames once, avoiding repeated prefix copies for fragmented frames. Rust registration writes reserve a single identity without holding the global connection map, so a blocked acknowledgement cannot stall unrelated registrations or listing.

These changes remove measured or directly demonstrated costs, not every possible bottleneck. Claude history and receipt indexes, and its journal replay at startup, still grow with the session lifetime; no receipt eviction is performed because it would weaken duplicate suppression. Pi/OMP turn checkpoints can still contain substantial active output. Listing still performs best-effort rmux/process discovery, host RPCs have finite deadlines, and rmux delivery deliberately retains process/UI checks and settling delays. Those checks protect delivery to the original terminal; removing them to lower latency would change correctness. No global performance optimum or end-to-end IM latency guarantee is claimed.


## Engine dispatch isolation and bounds

The production runtime has a fixed-load regression in `engine_runtime_fixed_load_preserves_isolation_and_admission_bounds`. Run it with:

```sh
cargo test -p agentix --bin agentix engine_runtime_ --all-features -- --nocapture
```

On September 9, 2026, with Rust 1.95.0 on macOS arm64, 64 independent conversations completed their replies in 15.66 ms while 31 host prompt acknowledgments remained withheld. The same test then held all 32 workers: the runtime admitted 224 additional pending requests and the upstream test channel held eight, after which the producer remained blocked. This verifies the 32-worker / 256-operation bound and upstream backpressure. [Recorded run](benchmarks/engine-dispatch.json).

This is one regression run with in-memory SQLite and local agent/channel fixtures. It proves that unrelated work can finish before a slow host is released and that admission remains bounded; it is not a provider throughput benchmark or an IM latency guarantee. Separate tests verify same-conversation order, independent working-card refresh, dynamic binding transfers, early replacement events, and shutdown fencing. Both mocked IM end-to-end suites now run the production scheduler, including their native bridge paths.

## Dispatch telemetry and overload handling

The `agentix::telemetry` tracing target emits structured Engine and control queue statistics every 30 seconds at INFO: pending and active counts, cumulative admitted/rejected/started/retired counts, total and maximum waiting and execution-slot times, and the oldest pending request's age. Slot time runs until worker retirement; DEBUG completion events include the handler's own duration and queue wait. Counter memory is constant. Queue counters reset on service restart. Queue rejection counts describe global-capacity refusals; per-conversation overload refusals appear in the persistent `rejected_inputs` count.

The source maintenance worker also reports `uncertain_inputs` and `rejected_inputs` from runtime SQLite, so those counts survive restart. Codex and Pi/OMP/Claude bridge timeouts emit WARN events with the backend and method; count these events by backend to identify transport timeouts. No prompt body or session history is added to telemetry. Enable detailed events with `[logging].level = "info,agentix::telemetry=debug"`.

Idle queues and queues with all workers occupied skip binding/action snapshot creation. When work can start, routes are resolved afresh, preserving attachment and replacement fences. This removes unnecessary cloning without retaining potentially stale snapshots; no unmeasured CPU speedup is claimed.

Each IM conversation can have 16 unfinished inbound operations, including its active operation. Excess requests are atomically marked rejected and represented by a coalesced, durable Session busy notice. They are not sent to the agent or replayed automatically. Resend rejected requests after the conversation queue clears. Duplicate pending event IDs use no extra quota; retirement releases quota. The global 256-operation / 32-worker bound and shared IM rate limits still apply. A workload occupying all workers can still backpressure new conversations.

The production flood regression holds one prompt and submits 300 further inputs to that conversation. It verifies that an independent conversation completes, exactly 15 following requests remain accepted, rejected requests cannot replay, and another request succeeds after the quota drains. The production outbox regression holds one IM send while both an existing and a newly staged notification reach other conversations, then checks the held conversation's delivery order.

## Slack transport

See the [Slack architecture and performance review](slack-review.md) for bounded rendering measurements, retained payload limits, and channel/method pacing tests. The reusable benchmark is `cargo +1.95.0 bench -p agentix-slack --bench render`.

## Codex proxy transport and observation

The local release benchmarks exercise the production proxy with reusable socket fixtures, without starting a model or touching the running shared app-server:

```sh
PROXY_BENCH_ROUNDS=3 cargo test --release -p agentix-codex --test proxy_performance -- --ignored --nocapture --test-threads=1
cargo test --release -p agentix-codex --test proxy_stdio_performance -- --ignored --nocapture --test-threads=1
cargo test --release -p agentix-codex --test proxy_registry benchmark_stream_notification_observation -- --ignored --nocapture
```

The transport matrix compares direct and proxied traffic across Unix→Unix, WS→Unix, WS→WS, and Unix→WS, using 300 small RPCs per client, 1/16 concurrent clients, and streams with 1 KiB, 16 KiB, and 1 MiB text fields. Every received payload is checked. For mixed transports, the direct baseline uses the upstream transport. Streaming samples send at least 8 MiB per client (32 MiB for 1 MiB messages). RPC p50/p99 measure round trips; stream p50/p99 measure receive interarrival, not end-to-end delivery latency. A separate paced test waits 10 ms between 100 RPCs and excludes that wait from timing.

Stdio compares direct JSONL over a local Unix socket pair with the production JSONL→Unix WebSocket adapter over the same frontend fixture. It does not measure terminal rendering or redirected disk I/O. Completion acknowledgments keep streaming fixtures alive until the receiver has consumed all data. Samples include framing, copying, observation, scheduling, and payload checks, but exclude model inference, IM delivery, remote network latency, and service startup. Matrix samples alternate direct/proxy order; stdio samples use direct then proxy. Benchmarks run sequentially to avoid competing benchmark loads; these are local measurements, not capacity guarantees.

The review removed an expired accept timer that added roughly 1 ms to each connection, added an early rejection path for method-first server notifications, grew full transfer buffers from 16 KiB to at most 128 KiB per direction, and avoided stdio output reserialization for already single-line JSON. Buffers grow only after a full read and do not wait to fill. Stdio still validates complete JSON and compacts multiline messages. Connection ownership changes still require complete validated responses; escaped method keys and reordered fields have regression coverage.

The opt-in timing regressions check average acceptance below 750 µs, median large WS transfer overhead below 2.5× direct, and notification observation below one third of full-value parsing. These thresholds reproduced the original costs on this machine, but can be noisy on other or heavily loaded hosts, so they are excluded from normal CI. Behavioral tests run normally.

Measured on September 11, 2026, macOS arm64 with Rust 1.95.0 in release mode. The following transport medians use three samples; acceptance aggregates the matching transport's sample averages. [Before raw samples](benchmarks/codex-proxy-before.json) and [after raw samples](benchmarks/codex-proxy-after.json) retain direct baselines, all loads, and repeated paced runs.

| Measurement | Proxy before | Proxy after |
| --- | ---: | ---: |
| Unix→Unix connection establishment | 1,235 µs | 73 µs |
| WS→WS connection establishment | 1,268 µs | 133 µs |
| WS→WS, 32 × 1 MiB text stream | 14.91 ms | 5.80 ms |
| Unix→WS, 32 × 1 MiB text stream | 26.25 ms | 20.53 ms |
| Stdio, 32 × 1 MiB text stream | 63.54 ms | 57.00 ms |
| Observe 20,000 × 16 KiB notifications (single release diagnostic) | 27.49 ms | 1.63 ms |

The same final run took 1.85 ms direct / 3.54 ms proxied for 300 small Unix RPCs, and 4.62 ms / 10.14 ms for WS RPCs: average added cost about 6 µs and 18 µs respectively. The mixed WS→Unix comparison added about 49 µs per RPC relative to Unix direct. These are warm, continuously active round trips. Paced runs showed median proxied round trips of 172–220 µs on Unix and 296–520 µs on WS; tails varied substantially (including a 14.9 ms direct-WS outlier and a 5.6 ms proxied-WS outlier). The benchmark does not attribute such outliers to a specific scheduler or networking cause and does not establish a hard latency bound.

Saturation overhead remains workload-dependent. For example, Unix→WS delivered the large stream in 20.53 ms versus 3.53 ms for WS direct, which also changes the frontend transport; stdio must still validate JSON and translate framing. These differences should not be described as zero overhead or identical throughput. The demonstrated accept timer, observation scan, small-copy and repeated serialization costs have been addressed. No remaining repeatable artificial delay was identified in these fixtures. This review does not measure production CPU/RSS, WAN loss, TLS, a live model's token rate, or arbitrarily many clients; those require separate representative measurements.

For small continuously active RPCs, the same final samples give the following per-request averages (median total time divided by 300):

| Mode | Direct | Proxy | Added time | Increase |
| --- | ---: | ---: | ---: | ---: |
| Unix→Unix | 6.2 µs | 11.8 µs | 5.6 µs | 91% |
| WS→WS | 15.4 µs | 33.8 µs | 18.4 µs | 119% |
| Stdio→Unix WS | 6.0 µs | 22.4 µs | 16.3 µs | 271% |

Percentages use a microsecond-scale local baseline and do not represent model response slowdown. Streaming throughput has different ratios; use the workload-specific raw samples rather than applying this RPC percentage to token generation.

### Medium and large message streams

These are single-client, continuous stream measurements from the same three release samples, not sequential request–response RPC timings. Each cell is the median total elapsed time for the entire stream. The size labels describe the text field; JSON framing adds bytes. Medium streams contain 509 WebSocket messages (16,461 bytes each) or 510 stdio messages (16,442 bytes each), totaling approximately 8 MiB. Large streams contain 32 messages totaling approximately 32 MiB.

| Text field / total stream | Mode | Direct | Proxy | Elapsed-time change |
| --- | --- | ---: | ---: | ---: |
| 16 KiB / ~8 MiB | Unix→Unix | 4.59 ms | 7.46 ms | +62.5% |
| 16 KiB / ~8 MiB | WS→WS | 2.43 ms | 1.86 ms | -23.6% |
| 16 KiB / ~8 MiB | Stdio→Unix WS | 8.40 ms | 12.70 ms | +51.2% |
| 1 MiB / ~32 MiB | Unix→Unix | 49.56 ms | 48.81 ms | -1.5% |
| 1 MiB / ~32 MiB | WS→WS | 5.92 ms | 5.80 ms | -2.1% |
| 1 MiB / ~32 MiB | Stdio→Unix WS | 30.14 ms | 57.00 ms | +89.1% |

Elapsed-time change is (proxy / direct − 1) × 100%; a positive value means slower transfer. For large stdio messages, the extra elapsed time is about 26.86 ms (+89.1%), corresponding to a throughput reduction of 47.1%, not 89.1%. That path includes full JSON validation and JSONL/WebSocket conversion.

Some WS samples finish faster through the proxy. Buffering, scheduling and run-to-run variation may contribute, but this benchmark does not establish their individual causes or a general proxy speedup. The approximately 1.5–2.1% differences in the large Unix and WS samples should be treated as roughly comparable results, not a guaranteed improvement. Medium/large RPC round-trip latency requires a separate request–response workload and cannot be inferred by dividing these stream totals by message count.

Source: [all final direct/proxy samples](benchmarks/codex-proxy-after.json); environment, reproduction commands and exclusions are documented above.

### Native Codex CLI against a mock app-server

The [native CLI benchmark](../crates/agentix-codex/tests/native_cli_performance.rs) runs the installed Codex TUI in a Python-standard-library PTY, comparing these local Unix paths:

- Codex CLI → mock app-server
- Codex CLI → production Agentix proxy → the same mock implementation

Run it explicitly; it requires Python 3 and an installed Codex CLI, but no model access:

```sh
PROXY_NATIVE_BENCH_ROUNDS=10 cargo test --release -p agentix-codex --test native_cli_performance -- --ignored --nocapture --test-threads=1
```

Set `CODEX_BENCH_BINARY` to select a particular installed CLI binary. Each sample uses a temporary working directory and CODEX_HOME. One completed warmup turn prepares the session; the driver then pastes a fixed prompt, waits for input handling, and measures from writing Enter to receiving the first and final answer markers in PTY output. These are TUI-output observations, not display-pixel timestamps or per-frame proxy timestamps. A separate launch-to-ready value is retained. The mock supports the TUI bootstrap RPCs and valid thread IDs, emits item-start/delta/completion events, and never calls a model.

The immediate workload returns a short answer without intentional generation delay. The paced workload emits 32 newline-terminated 69-byte chunks with a 10 ms sleep before each chunk, between first and completion markers. Actual sleeps can exceed 10 ms. Both paths use identical fixture behavior, including a 1 ms polling interval for new turns. Every sample must observe the complete answer; a timeout fails the run. Direct/proxy order alternates by round.

Measured September 11, 2026, macOS arm64, Codex CLI 0.154.0, release proxy, ten samples per path/workload (40 samples total):

| Workload / observation | Direct median | Proxy median | Median difference |
| --- | ---: | ---: | ---: |
| Immediate / first output | 11.822 ms | 11.651 ms | −0.171 ms |
| Immediate / complete output | 12.106 ms | 12.037 ms | −0.069 ms |
| Paced / first output | 21.140 ms | 21.472 ms | +0.332 ms |
| Paced / complete output | 404.916 ms | 401.251 ms | −3.664 ms |

Immediate complete-output samples ranged from 10.781–24.386 ms direct and 10.321–14.400 ms proxied. Paced complete-output samples ranged from 387.508–425.030 ms direct and 394.197–419.565 ms proxied. The overlap and negative differences do not establish a proxy speedup or zero cost. This run found no consistent user-visible slowdown for these two local workloads; scheduling, mock timing, PTY handling and TUI rendering dominate the microsecond transport cost. Ten samples are insufficient for reliable p99 estimates or a sub-percent regression guarantee.

[Raw native CLI samples](benchmarks/codex-proxy-native-cli.json) preserve every measurement. Use the protocol benchmarks above to isolate transport costs and exercise WS/stdio, concurrency and larger payloads. This native CLI comparison covers Unix→Unix only; native `--remote` does not accept `stdio://`. It does not establish performance with a real model, tool execution, a remote network, terminal emulator painting, or another CLI version.
