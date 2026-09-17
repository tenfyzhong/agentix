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
