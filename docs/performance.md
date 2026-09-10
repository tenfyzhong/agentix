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
