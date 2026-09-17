# PR #92 full review — 2026-09-17

## 📋 PR Summary

**Title:** fix: isolate background completion reads and order card writes
**Author:** tenfyzhong
**Branch:** fix/background-turn-content → main
**Files changed:** 36 | **Additions:** +4,655 | **Deletions:** -224
**Reviewed head:** abd9de76d316aa8c270280850d66448bdcd6310a
**Merge base:** 7b4aa188a5a5f987c7e98c4ac62ed53129dda4bc

The PR moves optional completion history reads and notification delivery out of Engine event dispatch. It introduces bounded background work, a shared card writer with revisions and uncertain-write isolation, transport progress tracking, and incremental registry reclamation. The review covers the full merge-base diff, including adapters, lifecycle integration, tests, and documentation.

Key changes:

- Four active completion workers, 64 total jobs, deduplication and cancellation.
- Cached or recovered turn content, loading/final cards and scoped Attach controls.
- Per-card ordering, definite/unknown outcome handling and replacement aliases.
- Idle cleanup, runtime restart handling, and sparse-container compaction.

## 📖 Review Guidelines

| Area | Priority | Checks performed |
| --- | --- | --- |
| Correctness | High | Completion snapshot/archival, card revisions, late writes, action tokens, attachment epochs |
| Lifecycle | High | Worker ownership, panic/cancellation, reload, runtime shutdown, weak-reference cleanup |
| Performance | High | Event admission, local writer cost, read/delivery coupling, serial recipient delivery, bounded maintenance |
| Adapters | High | Waiting versus dispatched markers, definite rejection versus uncertain outcome, final wire payload tests |
| Compatibility/docs | Medium | Cold-record defaults, changed asynchronous assertions, documented budgets and limits |

## 🚨 Issues Found

### P2: Loading delivery suspends the history deadline and retry

**Location:** `crates/agentix-core/src/engine/background_completions.rs:233–235`; recipient loop at line 370 and awaited delivery at line 426.

After the 100 ms loading delay, `run` awaits all loading-card deliveries before polling `read_with_retry` again. That future owns the history timeout and retry delay. A local queue or provider cooldown can therefore suspend timeout handling for up to the delivery budget per recipient. The underlying adapter may continue some independently spawned I/O, but the coordinator cannot consume its result, drop its read future on timeout, or initiate its retry during this wait.

A deterministic paused-clock reproduction starts one history read, holds loading delivery in the locally-waiting phase, advances six seconds and then 200 ms, and checks that the five-second timeout initiated a second read. It fails with `reads == 1`, expected `2`. This is an expected assertion failure, not a build or fixture failure.

The recipient loop is also serial: every recipient's final update waits for all loading deliveries and the history result. A slow transport can delay recipients on an unrelated transport. Four such jobs occupy all active slots, increasing optional notification latency and saturation loss. The Engine session lane remains available; this does not undo event-admission isolation. Serial fan-out also existed in the old synchronous path, but the new loading stage couples it to the new timeout/retry mechanism.

**Suggestion:** keep polling history and loading delivery concurrently inside the owned request. Give each recipient an ordered loading/final flow, with explicitly bounded fan-out, so a fast recipient can finalize without waiting for a slow one. Keep global admission limits, card revisions, cancellation ownership, and uncertain-send handling. Do not solve this by spawning detached tasks or blindly retrying initial sends.

**Reproduction:** `review_loading_delivery_must_not_suspend_history_deadline` in `render_tests.rs`. The follow-up fix enables it as an ordinary regression test:

```sh
cargo test -p agentix-core --lib review_loading_delivery_must_not_suspend_history_deadline -- --nocapture
```

No additional confirmed correctness blocker was found in the remaining reviewed paths. This is not a proof that no defects remain. Previously documented lifetime retention of uncertain-write aliases and lack of cross-process late-write fencing remain accepted limitations, not newly introduced findings in this review.

## 🧪 Test Coverage Analysis

| Test area | Type | Coverage |
| --- | --- | --- |
| Core engine/render | Unit + integration | Ordering, cancellation, cached/partial output, retries, deduplication, saturation, ownership and binding changes |
| Card registry | Unit | Idle reclamation, shared keys, reaper restart, bounded migration, retained aliases and writer drop |
| Domain/adapters | Unit + mock HTTP | Delivery phases, cooldowns, outcome classification, cache retention and Feishu wire notices |
| CLI runtime/Codex | Integration | Production dispatch progress/shutdown and asynchronous completion assertions |
| New review reproduction | Paused-clock unit | History timeout propagation while loading delivery waits locally |

Fresh validation ran `cargo test -p agentix-core -p agentix-domain -p agentix-feishu -p agentix-slack -p agentix-telegram --all-features`: **568 passed, 0 failed, 5 ignored**. The ignored cases comprise three performance benchmarks, the new known-failing reproduction, and an installed-Slack-CLI test. The reproduction was run separately and failed at its intended assertion. Formatting and whitespace checks passed. All 12 GitHub CI checks on the reviewed head were independently confirmed successful; that CI does not include the uncommitted reproduction.

New behavior has substantial automated coverage. Slow reads with fast transports and slow transports in isolation were covered, but their combination was missing. The new reproduction exposes that gap despite green CI.

Additional tests for the correction should cover a fast and a stalled recipient together, history completion during loading delivery, history timeout/retry while delivery waits, cancellation of both branches, and final-card ordering after a loading send has an uncertain result.

### Fresh performance measurements

Release profile on the current local macOS host, using the existing reusable fixtures. After the broad suite completed, two no-build-contention writer runs measured:

| Operation | Range, ns/op |
| --- | ---: |
| Reserve, 1 retained card | 22.3–22.7 |
| Reserve, 256 retained cards | 155.5–157.0 |
| Reserve, 4,096 retained cards | 201.0–208.3 |
| Reserve, 16,384 retained cards | 207.1–240.5 |
| Update, 1 KB body | 410.2–472.4 |
| Update, 1 MB body | 298.5–301.0 |

The first run overlapped the end of other validation and was noisier (for example, 255.4 ns at one retained card). It is not used to claim a regression. The larger body being faster in these short runs illustrates why small timing differences must not be treated as causal improvements. These results preserve the previously documented non-linear-scan behavior and lack of an unconditional large-body clone. They do not prove zero overhead relative to main.

The all-features admission run, 256 samples per scenario:

| Scenario | P50, µs | P99, µs | Pressure / completed reads |
| --- | ---: | ---: | --- |
| Cached | 19.29 | 27.50 | 0 rejected / 0 reads |
| Immediate history | 2.54 | 4.38 | 0 rejected / 256 reads |
| Stalled history | 3.46 | 17.04 | 4 active, 60 queued, 192 rejected / 0 completed reads |
| Duplicate stalled | 1.38 | 14.58 | 1 active, 0 queued, 0 rejected / 0 completed reads |

An immediately preceding run gave cached P50/P99 19.50/31.25 µs and stalled 3.46/16.08 µs. The cached numbers are substantially below older recorded runs; no controlled historical A/B was performed, so no speedup ratio is attributed to this PR from that difference. The repeatable conclusion is bounded, short event admission even with withheld history. Notification completion under blocked loading delivery has the separate reproduced problem above.

Commands:

```sh
cargo test --release -p agentix-core --lib card_write_performance -- --ignored --nocapture --test-threads=1
cargo test --release -p agentix-core --all-features --test engine benchmark_background_completion_admission -- --ignored --nocapture
```

## ❓ Questions & Uncertainties

- Real Feishu/Telegram/Slack latency and remote commit behavior were not exercised. Mock tests validate the client state machine, not provider delivery guarantees.
- Writer microbenchmarks measure local wrapper costs; they exclude serialization, network, timer contention and allocator tail latency. Admission benchmarks measure handler return time, not notification completion time.
- Retained mappings grow with uncertain writes. Incremental compaction releases spare capacity, not safety records; allocator behavior may keep process RSS high afterward.
- Maintenance bounds candidate count, not wall-clock duration. Hash-map growth can still rehash under the registry mutex. This is documented and was not established as a new measured regression.
- Existing comparison numbers use an earlier PR implementation, not a complete application A/B comparison against main. A universal “no performance regression” claim is unsupported.

## ✅ Review Verdict

| Aspect | Status | Notes |
| --- | --- | --- |
| Code quality | Yellow | Ownership and isolation are coherent; loading/read sequencing needs correction |
| Security | Green within reviewed scope | Scoped owner/generation/epoch checks retained; no new confirmed issue |
| Performance | Yellow | Fast local paths and bounded admission; confirmed avoidable notification delay under slow delivery |
| Test coverage | Yellow | Broad suite passes; new combined-slow-path reproduction fails |
| Documentation | Yellow | Extensive limits documented; five-second lookup wording overstates effective timeout during loading delivery |

**Overall at reviewed head: REQUEST CHANGES.** Correct the loading/read scheduling issue before accepting the notification latency behavior. This review did not change production code or publish a GitHub review.

## Follow-up resolution

The requested fix polls history and recipient delivery concurrently in the owned request, shares a single final view, and bounds recipient flows to four per job. Each recipient keeps loading/final order and existing uncertainty and revision checks. Queued recipients skip loading when the final view is available. No detached tasks are introduced. The original deadline reproduction and two additional regressions first failed against the reviewed head and pass after the correction. Further regressions cover queued recipients, cancellation, and an uncertain loading send without duplicate final delivery. The review and performance figures above describe the pre-fix head; post-fix validation is recorded in the background-completion design document.

## Second audit: 97c30e1

This follow-up reviewed the 38-file branch after bounded recipient concurrency was added. The findings below describe the pre-fix audit at 97c30e1; see the repair section below for the current local changes. Production code was unchanged by that audit. The local additions are two opt-in failing reproductions, one opt-in benchmark, and this report. The published head still has 12 successful CI checks; those checks do not include these local additions.

### P2: Scope validation does not survive transport admission waits

`card_writes.rs:240` validates a revision and binding before calling the adapter at line 260. The adapter may then wait in a conversation FIFO or provider cooldown. `DeliveryAttempt::dispatched()` records progress only; it cannot reject a stale binding, owner, generation, or revision. A scope change while the request is still known unsent therefore does not prevent eventual dispatch. Replacement sends have the same gap. Initial background sends at `background_completions.rs:459` likewise have no validation after adapter admission.

Two deterministic reproductions fail at the intended assertions:

- `review_binding_fence_must_survive_transport_cooldown`: poll an edit into a 30-second local cooldown, invalidate its scope, advance time, then observe one provider call instead of zero.
- `review_initial_send_must_recheck_owner_after_local_wait`: use the real background coordinator, hold a cached notice before dispatch, change the conversation owner, then release admission. The old notice is still sent.

This can publish stale content or obsolete controls after a binding/owner change. It does not establish that A overwrites a later successfully applied B on the same logical card: the existing writer mutex and uncertain-message replacement still protect that separate ordering property. Callback token scope validation also remains intact; these tests do not demonstrate execution of an unauthorized action. Requests already dispatched before invalidation are a different, unavoidable remote-outcome boundary.

Recommended fix: carry an owned validity/cancellation guard to the actual transport dispatch boundary. Recheck after FIFO admission and pacing, and before every retry/replacement network call. Invalidated, undispatched work must return a definite unsent/skipped result, preserving healthy targets and avoiding replacements. Apply the same rule to initial sends. A check only after the HTTP request returns cannot prevent stale delivery.

### P3: Attach-token membership adds a scan under the shared action lock

`background_completions.rs:358` uses `actions.iter().any(...)` to determine whether a token was consumed, even immediately after issuing a new token. The registry is already hash-indexed by token, but this path scans accumulated actions under the mutex shared with foreground interaction handling. The action registry predates this PR; the per-notice membership scan is new. Keeping valid historical buttons does not require a linear membership check.

The reusable `review_background_action_registry_scaling` benchmark runs 128 cached notifications with a mock transport for each prefilled registry size. A repeat after other tests completed measured:

| Prefilled action tokens | Notification P50, microseconds | P99, microseconds |
| --- | ---: | ---: |
| 1 | 16.96 | 22.58 |
| 1,000 | 18.00 | 23.25 |
| 100,000 | 193.62 | 572.04 |

The first run, overlapping validation, measured 34.54/134.00, 19.79/42.58, and 219.12/502.04 microseconds respectively. These are local cached-notification completion measurements, not Engine admission or real IM latency. Hash iteration order and scheduling affect the values; the source-level scan explains the scaling risk, while these measurements do not attribute every microsecond to that scan. Add a non-consuming constant-time token-membership API and use it here; do not shorten button lifetime just to hide the cost.

### Other checks and limits

| Area | Result |
| --- | --- |
| Query and loading scheduling | The prior timeout/retry issue remains fixed; both branches are owned and polled together |
| Queue and recipient pressure | Four active jobs, 64 admitted jobs, four recipient flows per job; excess notices remain intentionally lossy |
| Card ordering | Pending revisions coalesce; unknown remote writes retire their physical message; replacement aliases retain identity |
| Cancellation and reload | Read/delivery futures remain owned; shutdown and incompatible reload cancel optional work; compatible reload shares coordination |
| Resource reclamation | Healthy idle cards and sparse capacity are reclaimed incrementally; retired aliases remain intentionally retained for runtime safety |
| Transport outcomes | Local waits and definite rejections remain distinct from unknown wire outcomes; cooldown survives caller cancellation |
| Content and compatibility | Cold completeness field defaults safely; existing merge/visibility/elapsed-time behavior and rich-card notices remain covered |
| Additional confirmed issues | None beyond the P2 scope gap and P3 membership scan above within the inspected paths |

Fresh five-crate validation passed 573 ordinary tests. Core Clippy with warnings denied, formatting, and whitespace checks also passed. Both new scope reproductions were run separately and failed, then marked ignored with explicit reasons so ordinary suites remain usable. The benchmark is opt-in. No live provider or process-restart remote-write guarantee was tested. Existing retained-tombstone growth and lack of a durable notification outbox are documented limitations, not new findings.

Reproduction commands:

```sh
cargo test -p agentix-core --lib review_binding_fence_must_survive_transport_cooldown -- --nocapture
cargo test -p agentix-core --lib review_initial_send_must_recheck_owner_after_local_wait -- --nocapture
cargo test --release -p agentix-core --lib review_background_action_registry_scaling -- --ignored --nocapture
```

Recommendation: fix the pre-dispatch scope gap before merge and replace the token scan while touching this path. Neither finding requires abandoning bounded recipient concurrency or per-card serialization.

## Repair of the second audit findings

Both findings are addressed in the local follow-up. `DeliveryAttempt::run_if`
keeps a borrowed validator beside the transport future. Message-center admission
and each Feishu, Slack, and Telegram wire attempt await `dispatched()`, which
requests fresh validation. A bounded local handshake carries permission without
spawning another task. The existing total deadline includes validation waits;
cancellation drops the operation, validator, and handshake together.

Initial sends validate owner, backend generation, and the rendered action epoch.
Edits and replacements also validate the logical revision and their supplied
binding predicate. Invalidated attempts remain definitely unsent and do not retire
healthy card targets or trigger replacement sends. An explicit invalidated phase
lets the writer return without another validation await outside the budget.
Already-dispatched remote requests remain outside this prevention guarantee.

Attach-token membership now uses `HashMap::contains_key` through a non-consuming
registry API. Existing token reuse, consumption, and callback scope checks remain.

The two audit reproductions are enabled as ordinary passing tests. Additional
coverage checks rejected-retry invalidation, blocked-validation timeout and future
drop, healthy-target reuse, and avoiding post-rejection validation waits. The last
case was confirmed failing before the explicit invalidated phase was added.

The same release cached-notification fixture now measured P50/P99 of
18.67/34.25 microseconds at one token, 18.67/30.96 at 1,000, and 18.25/30.79 at
100,000. The previous 100,000-token result was 193.62/572.04 microseconds. This
removes the observed registry-size scaling; the mock fixture does not measure
real adapter admission, provider latency, or the full checkpoint overhead.

A further deterministic race test invalidates the card revision while the scope
validator is waiting. It failed before the final revision check was moved after
the awaited predicate, and is included in the ordinary core test suite.

Validation: the five affected crates passed 578 tests with all features. After the
final revision-order adjustment, all 110 core library tests passed, including the
new race regression. All-target/all-feature Clippy passed for the five crates and
was rerun for core after that adjustment; formatting and whitespace checks passed.
Changes remain local and uncommitted; published CI does not cover this follow-up.
No live IM delivery was exercised.

## Dispatch-handshake audit

A deterministic poll-level test exposed a remaining local scheduling gap when
Tokio's cooperative budget was exhausted: validation could succeed, yet dispatch
would resume on a later poll. The handshake now directly re-polls the transport
after granting permission and disables cooperative yielding only while receiving
that permission. Admission and the validator retain ordinary cooperative behavior.
A 512-checkpoint regression verifies that this does not make repeated dispatches
monopolize the task. All nine delivery tests pass, including the new failing-then-
passing exhausted-budget regression, rejected retries, and blocked-validation
cancellation. No additional worker, timer, or retained registry is introduced.

The review also followed the current Feishu token-refresh loop, Slack and Telegram
pacing/retry paths, the shared FIFO, and card-writer cancellation classification.
Adapters continue to checkpoint each attempted provider call. In-flight outcomes
remain conservative: the checkpoint precedes polling the provider future and is
not a network-byte or cross-thread atomicity guarantee.

| Requirement checked | Current evidence |
| --- | --- |
| Old card revisions cannot overtake newer applied state | Writer mutex, late revision check, uncertain-target isolation, render race regressions |
| Invalid scope during local admission skips delivery | Owner/cooldown regressions and dispatch validator at each instrumented boundary |
| Cancellation and retry retain safe outcome classification | Nine domain delivery tests, card target-reuse test, adapter pacing tests |
| Query progress and bounded resource ownership | Independent-history, bounded fanout, pressure, panic-release and reload tests in core |
| Registry reclamation and alias safety | Idle cleanup, compaction, retained-alias and reaper lifecycle tests |
| Attach compatibility and lookup cost | Existing consumed-token/epoch regressions and hash membership implementation |
| Scope of validation | Local deterministic tests and provider HTTP mocks; live IM and process-restart delivery remain unverified |

Final local validation for this audit passed 582 tests across core, domain,
Feishu, Slack, and Telegram with all features; all-target Clippy with warnings
denied, rustfmt, and whitespace checks passed. The latest release fixture measured
P50/P99 36.54/85.17 microseconds (one token), 32.79/84.71 (1,000), and
18.83/34.67 (100,000). Warmup and scheduling noise affect these short samples;
the large-registry scan does not recur. This fixture does not isolate checkpoint
cost or establish real-provider latency. No other confirmed issue was found in
the inspected paths. All audit/fix changes remain local and uncommitted.
