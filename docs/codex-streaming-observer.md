# Bounded Codex proxy observation

The proxy now observes WebSocket frames incrementally instead of assembling every message in tungstenite. A large history response no longer permanently disables session discovery. Raw bytes are still forwarded unchanged; the observer never injects Pong/Close frames or terminates a healthy transport because of a JSON error.

## Ownership and resource boundaries

- Full text messages up to 64 KiB retain the existing registry callback path. Above that threshold, an incremental JSON validator retains only lifecycle metadata: request ID/method, result status, thread ID/ephemeral/source-subagent presence, and the relevant request parameters/client name. Field ordering and escaped keys are supported. Discarded strings, arrays and objects are still validated before ownership changes are published.
- The frame decoder retains a 14-byte header and uses 8 KiB of scratch space for masked input. Message text buffering is capped at 64 KiB. Projection has a 128-container depth budget and 4 KiB key/scalar-token budgets, with a fixed set of retained metadata fields. These are semantic budgets, not a limit on history/tool-output message length. Messages exceeding a semantic budget are ignored; a later valid message remains observable.
- Frame fragmentation, arbitrary read boundaries, masks, binary messages and interleaved control frames are handled independently of JSON parsing. No allocation uses the declared wire length.
- Invalid JSON or UTF-8 is discarded at the message boundary and does not disable the next message. Invalid frame structure disables observation for that direction and removes the connection from the registry, so stale sessions are not offered as attachable. Raw forwarding continues. Re-establishing the transport creates a fresh registration; the proxy does not forcibly reconnect the user's CLI.
- Large projected notifications retain content/completion wakeups, and projected response envelopes preserve acceptance/resolution of previously recorded answers. They are **not** replayed as complete interactive requests or answers: the omitted body cannot be reconstructed from lifecycle metadata. Full-fidelity proxy observation of interactive payloads larger than 64 KiB is outside this change; their original bytes still reach Codex. This avoids manufacturing truncated questions or approvals.

Regression tests cover a 65 MiB lifecycle response, no pre-existing IM attachment, replacement-only listing, successful replacement attach, rejection of stale attach, split UTF-8/surrogate escapes, masks/fragmentation/control frames, invalid-message recovery, and framing-loss registry cleanup. These use isolated transports and protocol fixtures, not live Telegram or a model session.

## Measurement method

Measured October 27, 2026 on Apple M3, macOS 27.0.1 arm64, Rust 1.95.0, optimized release builds. Five rounds alternate implementation order. No other agent-launched test/build workload ran during accepted benchmark measurements. Figures below are medians of the five per-run results; they are observations, not confidence intervals or capacity guarantees.

Two distinct comparisons avoid treating a broken observer as a fast one:

1. **Observer plus registry:** the test-only `legacy_bench.rs` preserves the previous buffered observer with the temporary 64 MiB frame/message budget. Both implementations run from the same test executable in fresh processes. Input is generated in bounded 16 KiB blocks. Every valid row verifies registration. The 16-connection case is round-robin across resident observers, not parallel network clients. Each small case has 10,000 messages; each large case has 20, so large-case P99 is effectively the sample maximum. CPU time is process CPU during the sample; peak RSS includes the entire test process, not exclusively the observer.
2. **Complete relay:** the baseline production code is `d7c1d7f` (the original 16 MiB observer), with only the identical benchmark fixture and CPU-time dev dependency copied into its worktree. Payloads are below its valid limit. The matrix covers Unix/WS frontends and upstreams, 1/16 concurrent clients, small RPCs and streamed notifications. Every sample creates a session before traffic and verifies another successful registration afterward. CPU includes mock peers, runtime and proxy. RPC timings are round trips; streaming timings are receive interarrival, not end-to-end message latency. Direct controls are retained in the raw data.

The first streaming implementation regressed WS-to-WS 1 MiB throughput by about 56%. Replacing a per-byte short-circuit control-character scan with a vectorizable reduction removed that cost while preserving validation. A later relay attempt accidentally reused a baseline binary through a shared Cargo target directory and was discarded. Accepted relay data verifies distinct implementation markers and records distinct binary SHA-256 hashes. Reproduction should use separate target directories.

## Observer measurements

| Payload | Resident clients | Median latency old → new (µs) | CPU change | Peak RSS old → new (MiB) |
| --- | ---: | ---: | ---: | ---: |
| 128 B | 1 | 10.00 → 1.08 | -86.3% | 4.00 → 3.78 |
| 128 B | 16 | 5.67 → 1.38 | -71.9% | 5.95 → 3.86 |
| 1 KiB | 1 | 10.83 → 1.29 | -85.1% | 4.02 → 3.78 |
| 16 KiB | 1 | 14.67 → 4.12 | -69.4% | 4.03 → 3.81 |
| 1 MiB | 1 | 307.67 → 61.88 | -79.0% | 5.59 → 3.56 |
| 17 MiB | 1 | 5456.00 → 992.79 | -82.4% | 37.75 → 3.55 |

At 65 MiB, all legacy runs lost observation (`tracked=false`); their apparent throughput is invalid and is excluded from percentage comparisons. The streaming observer retained registration in all five runs, with median processing latency 3.78 ms and peak RSS 3.56 MiB.

## Complete relay measurements

The largest observed throughput reduction was 4.5% for ws-to-unix, 1 client(s), 1024 KiB traffic: median interarrival 1,635 → 1,719 µs (+5.1%), P99 1,786 → 1,920 µs (+7.5%), CPU -5.9%. Across the matrix, the largest P99 increase was 20.9% (215 → 260 µs). CPU changes ranged from -33.0% to +5.4%. These differences are retained rather than asserted to be noise. This does not prove zero regression on other hardware, escaped-heavy JSON, WAN links or production traffic.

| Front → upstream | Clients | Traffic | Throughput change | P99 old → new (µs) | CPU change |
| --- | ---: | --- | ---: | ---: | ---: |
| unix → unix | 1 | 128 B RPC | +31.6% | 15 → 12 | -24.2% |
| unix → unix | 1 | 1 KiB stream | -2.0% | 11 → 12 | -4.2% |
| unix → unix | 1 | 16 KiB stream | -0.8% | 30 → 29 | -8.9% |
| unix → unix | 1 | 1024 KiB stream | -0.6% | 1659 → 1651 | -6.4% |
| unix → unix | 16 | 128 B RPC | +29.1% | 178 → 130 | -33.0% |
| unix → unix | 16 | 1 KiB stream | +2.2% | 116 → 113 | -3.7% |
| unix → ws | 1 | 128 B RPC | +21.2% | 40 → 35 | -17.7% |
| unix → ws | 1 | 1 KiB stream | +0.4% | 22 → 17 | +1.6% |
| unix → ws | 1 | 16 KiB stream | +8.5% | 38 → 31 | -5.3% |
| unix → ws | 1 | 1024 KiB stream | +6.5% | 1005 → 967 | -5.7% |
| unix → ws | 16 | 128 B RPC | +37.6% | 252 → 176 | -28.3% |
| unix → ws | 16 | 1 KiB stream | +5.5% | 192 → 191 | -5.7% |
| ws → unix | 1 | 128 B RPC | +0.7% | 94 → 85 | -12.9% |
| ws → unix | 1 | 1 KiB stream | +4.5% | 15 → 14 | -8.5% |
| ws → unix | 1 | 16 KiB stream | +3.6% | 33 → 32 | -8.8% |
| ws → unix | 1 | 1024 KiB stream | -4.5% | 1786 → 1920 | -5.9% |
| ws → unix | 16 | 128 B RPC | +8.5% | 263 → 268 | -22.9% |
| ws → unix | 16 | 1 KiB stream | +4.0% | 142 → 135 | -5.2% |
| ws → ws | 1 | 128 B RPC | +0.7% | 48 → 47 | -5.9% |
| ws → ws | 1 | 1 KiB stream | -1.5% | 23 → 23 | -4.7% |
| ws → ws | 1 | 16 KiB stream | +0.9% | 22 → 24 | +5.4% |
| ws → ws | 1 | 1024 KiB stream | +8.0% | 401 → 374 | -6.4% |
| ws → ws | 16 | 128 B RPC | +14.8% | 241 → 211 | -17.5% |
| ws → ws | 16 | 1 KiB stream | +12.5% | 215 → 260 | -7.0% |

## Reproduce and inspect

Raw samples: [observer](benchmarks/codex-streaming-observer.jsonl), [relay including direct controls](benchmarks/codex-streaming-relay.jsonl). Payload sizes in raw rows include protocol JSON overhead where applicable. The repository contains the benchmark implementations and orchestration scripts; no developer daemon or credentials are required.

```sh
cargo test -p agentix-codex --all-features
cargo test --release -p agentix-codex --lib observer_benchmark --no-run
python3 scripts/benchmark-codex-observer.py /absolute/path/to/release/library-test-binary

# Build identical proxy_performance.rs fixtures against baseline and current
# production code in separate worktrees AND separate CARGO_TARGET_DIR values.
# The baseline fixture also needs cpu-time = "1.0.0" in dev-dependencies.
cargo test --release -p agentix-codex --test proxy_performance --no-run
python3 scripts/benchmark-codex-relay.py /absolute/baseline-binary /absolute/current-binary
```

The relay runner rejects identical binary hashes. Failed tests, missing matrix rows and missing tracking verification fail the runner rather than becoming performance data. No deployed Agentix service or live Telegram acceptance was exercised.
