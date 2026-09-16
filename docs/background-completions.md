# Background completions

## Decision

Move optional background content lookup and notification transport I/O out of the Engine session dispatch lane. This fits the existing owned asynchronous input/prompt machinery and preserves adapter boundaries. The Engine must finish the local turn before starting optional I/O; neither history nor IM transport may delay subsequent session events.

## Ownership and architecture

| Component | Owned state | Responsibility |
| --- | --- | --- |
| Engine completion handler | Terminal buffer and local cleanup | Capture the turn, archive it, finish draining, release the delivery permit |
| Background coordinator | Bounded jobs, recent keys, captured recipients | Read history, render loading/final content, cancel optional work |
| Card writer | Logical-card revisions, control barrier, physical aliases | Serialize edits, discard stale states, isolate uncertain remote writes |
| Transport attempt | Waiting/dispatched phase and deadlines | Distinguish unsent work from unknown remote outcomes |

The background coordinator owns tasks; the card writer runs within its caller and
adds no second worker pool. Provider pacing stays inside adapters. The one-shot
completion permit preserves local cleanup ordering without another Engine event
protocol. Existing renderers and scoped action validation remain shared.

## Required invariants

- Snapshot the exact session, turn, terminal status, error, recipients, output policy and existing draining message. Never resolve a destination using the current attachment when a lookup completes.
- Prefer the existing hot/cold turn buffer. Reuse the normal output formatter so reasoning/tool visibility and block ordering remain consistent. Partial cached output must not be represented as a complete history snapshot.
- Own all reads and sends in a bounded coordinator. Deduplicate in-flight turn keys and retain a bounded recent-completion window. No unbounded spawned tasks or semaphore waiters. Saturation must be explicit and observable rather than blocking event ingestion.
- Render fast results directly. When a read is slow, publish a loading card and update its logical card after lookup; an uncertain transport update retires the physical message and uses a replacement. Persistent lookup failure settles the loading state when delivery succeeds.
- Preserve draining-card identity when safe. Fence an old draining result against attachment changes, replacement, backend generation changes and card replacement. Background cards created by the task are separate from active turn views.
- Shut down owned workers on runtime stop/drop. Configuration reload must not resurrect obsolete reads or apply old output policy to current cards.

## Validation gates

Before production changes, reproduce blocked completion handling with a reusable test. Cover cached/uncached output, slow/error/missing history, retry, duplicate and out-of-order completions, queue saturation, multiple recipients, draining/attach/last races, reload, backend generation change and shutdown. Exercise the production scheduler, not only direct Engine calls.

Use repeatable local release-mode workloads for cached completion, immediate history and withheld history under event bursts. Measure admission latency, independent-event progress, lookup counts, pending/active bounds and notification completion. Compare iteration results and document fixture limitations. Optimize evidenced bottlenecks while preserving the invariants; no universal optimum or live IM/provider guarantee is claimed.

## Runtime behavior

The completion handler restores local state, records the terminal status and elapsed time, releases only the matching active turn, revokes its Stop action, and archives its buffer. Draining subscriptions are released without waiting for history or IM transport. The optional worker owns the original session and turn IDs, recipient identities, output policy, and message IDs; it never selects an update destination from the current attachment.

A cached nonempty prompt and a completed answer item (or terminal history summary) are sufficient for the normal completion notice. Subsequent answer deltas invalidate that completeness marker; it survives cold-cache storage. They represent locally observed output, not an independently verified complete event ledger. Missing prompt, missing answer, or an answer with only streaming fragments triggers a history lookup; merging preserves the existing output formatter, process visibility, and ordering. Fast results produce one final notice. If the read has not completed after 100 ms, the worker sends a loading notice (or edits the original draining message), then updates that logical card with the result. An uncertain edit uses a replacement physical message; failed or uncertain initial/replacement sends are not blindly retried.

| Boundary | Limit or policy |
| --- | --- |
| Running optional jobs | 4, including reads and delivery |
| Total admitted jobs | 64; overflow skips the optional notice and logs a warning |
| Deduplication | Exact session/turn keys while queued/running; 256 recent keys |
| History | At most 2 attempts; 5 seconds for each complete paginated lookup; 100 ms between attempts |
| Pagination | 20 turns per page; repeated cursors terminate lookup |
| Delivery | 5 seconds per dispatched remote request; 60 seconds total admission/retry budget per operation; 180 seconds per background recipient including writer wait; one recipient failure does not suppress the others |
| Subagent detection | 5 seconds; failed detection skips standalone notices |
| Optional title | Existing shared title reader; background wait up to 50 ms |

The public `background_completion_statistics()` reports active, queued and rejected counts without reading SQLite or asking an adapter. Saturation is deliberately lossy: the queue is not a durable outbox, and process restart does not replay its pending work. Delivery duration scales with recipient count. A timeout means missing optional content, not failure of the completed agent turn.

Results require the same backend generation and recipient owner. Updates to existing draining cards additionally require the captured binding epoch. An independently created background card remains its own destination across later attachment switches; its final Attach action is minted with the current epoch. Compatible Engine snapshots share the coordinator; changes to notification policy, output visibility or transport cancel old work. Runtime shutdown, runtime-task abort and final owner drop cancel workers; a worker panic releases its slot.

## Card write ordering

All Engine card edits and action disabling use a shared `CardWrites` coordinator through `OrderedChannel`. Live output, working-duration refreshes, interaction cards, queued-input receipts and background completions therefore share one asynchronous writer per logical card. Different cards remain independent at this layer; adapters retain their existing outbound rate-limit queues. No extra delivery tasks are spawned by this coordinator.

A monotonically increasing revision identifies each pending state. Background work reserves it only after admission and before history lookup; duplicate/rejected jobs cannot invalidate accepted work. Loading and final content retain the same revision. Live render snapshots reserve before asynchronous rendering work. At the writer boundary, outdated revisions are discarded, coalescing waiting intermediate states. Background binding/owner/generation checks run again after acquiring the writer and before a replacement send.

Button disabling records a separate revision barrier. It does not invalidate pending body content: pending views through that barrier have their actions disabled, while a newer view can introduce fresh controls. Applied revisions prevent an old disable request from removing a newer view's controls.

`DeliveryAttempt` tracks local queue/cooldown waiting separately from remote dispatch, in the caller task. MessageCenter, Telegram, Feishu and Slack mark those boundaries; uninstrumented adapters conservatively count as in flight. An operation has a 60-second total admission/retry budget and each dispatched request has a five-second response budget. Known rate-limit rejection resets the phase to waiting without discarding the shared cooldown.

Before polling an edit, the writer reserves its target. Success restores it. Local wait cancellation or `NotSent`, `Rejected`, and local `InvalidPayload` errors also restore the target, allowing later updates. A remote timeout, transport error or in-flight cancellation retires the original message: dropping the client future does not prove that the server stopped applying it. The latest state is sent as a new physical message, and both message IDs resolve to the same logical writer. Later updates and callbacks use the replacement, so a late commit against the old ID cannot overwrite its content. Disabling actions also shares the writer; it never recreates an uncertain card merely to remove controls. Feishu retains its cached view until disabling succeeds so an explicit rejection remains retryable.

A definite replacement rejection leaves the original message retired but permits a later safe replacement attempt. If the replacement send itself has an unknown result, automatic replacement attempts stop rather than risk repeated sends. The old card can remain visible, and a delivery failure can still prevent final content from reaching IM. This is best-effort delivery, not an exactly-once outbox or a remote compare-and-swap protocol.

Engine reloads share writer revisions and replacement mappings, including across notification-policy changes. Healthy idle card entries are pruned incrementally (at most eight candidates per lookup once the cleanup queue reaches 256 entries); revisions held by queries remain alive. Lookup hits borrow their map key. Cleanup rotates live and retained candidates for later inspection, including cards temporarily retained during a write. Retired message IDs and replacement mappings are retained for the runtime lifetime so old handles cannot become writable again. Memory for these mappings is proportional to uncertain writes. They are not persisted across a full process restart; this change's ordering guarantee applies within a runtime and its reloads, not to unknown remote requests surviving a process restart.

## Completion cleanup and controls

Preparation snapshots terminal content and reserves admitted card revisions before local archival. Optional work waits for a permit released only after archival and draining cleanup succeed. Cancellation or a local cleanup error drops the permit and skips the notice. Disabled transports inherited during reload are skipped individually; they cannot stop local cleanup or delivery to enabled recipients.

Loading and final views reuse one Attach token while the recipient owner, backend generation and binding epoch remain unchanged. Failed or cancelled edits preserve the visible token. Consumed tokens are never recreated for that epoch. A changed binding produces a new scoped token, and an already attached target renders disabled `Attached`. The writer rechecks the captured binding epoch before dispatch. Unknown remote outcomes retain the applicable tokens, whose normal owner/generation/binding checks still govern execution.

Loading and missing-content explanations appear in both fallback text and structured sections, including Feishu cards with cached prompt/process content. Final views remove the loading section.

## Regression coverage

Reusable Engine tests cover blocked reads, finite retries, loading-card identity across attachment switches, final Attach actions, cached content, missing content, failed status, overlapping turns, shared queries with independent recipient failure, queue saturation, duplicate events, generation changes and reload cancellation. Private lifecycle tests verify terminal cached state, elapsed time, Stop-token removal and preservation of a newer active turn. The production `run_engine_loop` test holds a history request open, admits subsequent events in the same session, and verifies worker cancellation when the loop is aborted while the Engine remains referenced.

The benchmark is an ignored integration test, not a production-network test:

```sh
cargo test -p agentix-core --release --all-features --test engine benchmark_background_completion_admission -- --ignored --nocapture
```

It measures 256 completion-handler admissions per scenario using deterministic adapters and isolated SQLite state. Cached and immediate-history cases settle each optional job between samples; stalled and duplicate-stalled cases measure a burst without releasing the history gate. Reported read counts count completed mock reads, so stalled scenarios report zero even while requests are active. Setup, user-visible network delivery latency and remote-server behavior are outside these measurements.

## Local measurements (2026-09-16)

Apple M3, macOS, Rust 1.95.0, release profile. The initial asynchronous implementation still loaded a cold record again after the completion handler had already restored it. Removing that duplicate read reduced the first measured cached-admission P50 from 172.33 to 168.21 microseconds. After the cache-completeness fix, the final run plus three repeats produced these ranges:

| Scenario | P50, microseconds | P99, microseconds | Pressure / completed reads |
| --- | ---: | ---: | --- |
| Complete cached prompt and answer | 165.54–167.58 | 198.21–208.17 | No rejected jobs; zero history reads |
| Immediate history result | 2.46–2.58 | 4.08–10.29 | No rejected jobs; 256 reads |
| 256 distinct turns with history held open | 3.46–3.62 | 15.50–16.42 | 4 active, 60 queued, 192 rejected |
| 256 copies of one turn with history held open | 1.42 | 13.38–14.04 | 1 active, 0 queued, 0 rejected |

The cached fixture restores and archives actual local turn state; the uncached fixture has no turn buffer to archive. This accounts for their different admission costs. The largest cached sample across the final repeats was 786.04 microseconds. These small local differences are subject to scheduling noise; the important result is that withheld remote history does not extend completion admission to the lookup deadline. The original synchronous regression exceeded its 100 ms completion-handler budget; the asynchronous path passes it and the production same-session progress test.

Further changes to cold storage or more worker concurrency were not justified by this workload: admission remains below a millisecond, remote work stays within fixed limits, and increasing concurrency would increase pressure on the failing provider. This is a measured operating point, not proof of a universal optimum. End-to-end real-provider and real-IM latency still requires deployment observation.

Initial background-completion validation: 291 Engine integration tests, 50 Engine module tests and 10 runtime tests passed; the opt-in benchmark passed independently. Clippy passed for `agentix-core` and `agentix` with all features and targets, with warnings denied. Additional focused regression checks cover the partial-content fallback notice.

Card-ordering regressions additionally cover A in flight while B/C wait (only C is sent), remote A committing after client cancellation or timeout, replacement callbacks sharing revisions, configuration reload, old query results, duplicate background requests, independent-card progress, cancelled waiters, action disabling, binding changes while waiting and uncertain replacement sends. The first serialization and cancellation regressions failed against the earlier implementation before the writer was introduced. These tests simulate remote commits independently of the client future; they do not claim live-provider timing validation.

Card-ordering validation: 401 core tests (including 292 Engine integration tests and 10 new ordering regressions) and 10 production runtime tests passed. Clippy passed for core and CLI with all features and targets and warnings denied.

Release admission recheck after card ordering: cached P50/P99 166.50/210.21 microseconds; immediate history 2.54/3.46; stalled history 3.42/16.54; duplicate stalled 1.38/14.62. The stalled burst retained the same 4 running / 60 queued / 192 rejected bounds. This benchmark covers event admission, not IM network latency or remote write ordering.

Follow-up review (2026-09-17) found three regressions despite the earlier green suite. See [Card ordering follow-up review](background-completions-review.md) for failing tests, severity and required corrections. The current implementation is not ready for acceptance.

## Delivery semantics follow-up (2026-09-17)

After the admission/dispatch and control-barrier fixes, 541 tests passed across core, domain, Feishu, Telegram and Slack, plus 10 production runtime tests. The three original review regressions and the additional Feishu cache-retention and Slack malformed-response regressions pass. Tests use local mocks, not live credentials.

A repeat of the same release benchmark after compilation completed measured P50/P99 of 168.25/203.42 microseconds for cached completion, 2.62/4.08 for immediate history, 3.42/15.92 for stalled history, and 1.42/13.92 for duplicate stalled history. The stalled burst retained 4 active jobs and 60 queued jobs, rejecting 192; duplicates retained one active job. A run concurrent with other build work measured cached P50/P99 of 177.67/292.21 microseconds. These small local samples demonstrate bounded admission in the fixture, not a provider latency guarantee or a precise overhead percentage.

## Card writer performance follow-up (2026-09-17)

The ordering fix introduced measurable local CPU costs: full-registry scans above 256 entries, unconditional deep copies of each outbound view, and waiting for a busy writer even when a revision was already obsolete. Regression tests first reproduced all three behaviors. The writer now limits cleanup work, borrows unchanged views, and rejects already-obsolete revisions before acquiring the async lock. It still rechecks the revision and binding after acquiring the lock, and copies views only when enabled buttons must be disabled.

Repeat the microbenchmarks with:

```sh
cargo test --release -p agentix-core --lib card_write_performance -- --ignored --nocapture --test-threads=1
```

Apple M3, macOS, Rust 1.95.0; median of three process runs, nanoseconds per operation. Baseline is PR commit `907f846` plus the same benchmarks. Each reserve scenario holds all card revisions alive and measures 20,000 lookups of one existing card; setup is excluded. Each update scenario measures 5,000 writes to a no-op adapter, including delivery tracking but excluding serialization and network I/O. These are throughput averages, not latency percentiles.

| Operation | Before | After |
| --- | ---: | ---: |
| Reserve with 1 retained card | 54.3 | 18.8 |
| Reserve with 256 retained cards | 235.3 | 207.2 |
| Reserve with 4,096 retained cards | 6,399.0 | 224.0 |
| Reserve with 16,384 retained cards | 25,864.8 | 239.7 |
| Update with 1,024-byte body | 364.3 | 305.1 |
| Update with 1,000,000-byte body | 15,203.6 | 303.7 |

The large-registry lookup is about 108 times faster and the large-body wrapper update about 50 times faster in this fixture. Normal update copying no longer scales with body size. Cleanup visits at most eight candidates; map growth can still allocate and rehash, so this is not a hard real-time latency guarantee. The cleanup queue and index share each immutable message key through `Arc`; the queue does not duplicate its strings. A resource follow-up adds idle cleanup as described below. Safety mappings remain proportional to uncertain writes and are not evicted merely to meet a memory cap.

These results compare two versions of the ordering implementation, not the whole application against synchronous delivery. Version tracking, per-card serialization and timeout instrumentation retain a small correctness cost. Background work remains limited to four active and 64 total jobs; incomplete histories may require a loading card and one retry. Increasing those limits would put more load on a stalled provider. Real IM throughput, provider latency and sustained-outage memory usage are not established by these local microbenchmarks; no universal optimum is claimed.

### Idle resource reclamation

The card registry shares `Arc<MessageRef>` keys between its index and sweep queue. Hot lookups continue to inspect at most eight candidates above the 256-entry threshold. On first access from a Tokio runtime, the writer starts one cleanup task that checks at most 256 candidates every second, even without subsequent messages and below the hot-path threshold. Candidates still in use or retained for uncertain delivery remain in the rotation; idle healthy cards are removed. Empty registries release both map and queue capacity. Reclamation is eventual and depends on runtime scheduling; large registries take multiple ticks.

The task holds only a weak registry reference between ticks and is aborted when the writer is dropped. Engine reload shares the existing writer and its cleanup task. If the old Tokio runtime has shut down, the next registry access on a running runtime replaces the finished cleanup task. The existing registry mutex serializes initial startup and restart, so concurrent callers cannot create multiple live cleanup tasks. No extra task is created for synchronous-only usage outside a Tokio runtime. The safety mappings for uncertain writes remain retained; this follow-up does not change late-write isolation or claim bounded memory during an indefinitely failing provider.

Resource follow-up validation: both sharing and idle-reclamation tests failed before implementation. Four new tests cover shared key storage, idle reclamation and capacity release, bounded batches preserving live/retired cards, and cleanup-task cancellation on writer drop. All 427 core tests pass. Three release runs of the same fixture produced medians of 21.5 / 147.6 / 185.8 / 203.4 ns for reservation at 1 / 256 / 4,096 / 16,384 retained cards, and 297.9 / 282.4 ns for 1 KB / 1 MB updates. These remain local microbenchmarks, not IM latency or timer-contention measurements.
