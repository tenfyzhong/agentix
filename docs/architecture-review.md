# Architecture optimization review

The initial review covers cross-session I/O isolation, Engine responsibility separation, dependency boundaries, and removal of the unused Pi implementation. The follow-up below covers dispatch telemetry, snapshot overhead, conversation admission fairness, and durable notification isolation.

| Requirement | Final implementation | Acceptance evidence |
| --- | --- | --- |
| Isolate slow I/O | Engine and control use bounded admission. Engine reserves conversations and qualified sessions, including displaced routes during transfers. Backend fences protect unknown replacement IDs. Working refreshes are coalesced per turn. IM queues preserve per-conversation order and shared pacing. | [Runtime tests](../crates/agentix/src/main.rs), [routing tests](../crates/agentix-core/tests/engine.rs), and [queue tests](../crates/agentix-core/src/dispatch.rs) cover blocked hosts, slow IM delivery, limits, ordering, transfers, opaque buttons and recovery barriers. Both IM end-to-end suites use the production runtime. |
| Separate Engine responsibilities | SessionService owns discovery and durable bindings. TaskBoardService owns task input and conversation recording. Turn, interaction and workspace state have separate coordinators. Focused workflow modules coordinate these owners; presentation functions construct views independently of I/O. | [Session-service and presentation tests](../crates/agentix-core/src/engine.rs) run without an Engine or channel. [Task-service tests](../crates/agentix-core/src/engine/task_board/tests.rs) record qualified session messages without an Engine or IM connection. |
| Enforce dependency boundaries | agentix-domain owns contracts and pure state rules; agentix-storage owns runtime SQL and turn caching; agentix-core owns application services. Adapters depend on domain contracts. | [Architecture tests](../crates/agentix/tests/architecture.rs) enforce production dependencies. Cargo dependency trees confirm domain/storage do not depend on application or host adapters. Adapter test-only dependencies on core remain intentional. |
| Remove unused code and duplicate assembly | The legacy agentix-pi crate is absent from the workspace, filesystem and lockfile. Pi/OMP use the shared bridge. The executable and integration fixtures import one library runtime. An unused direct domain dependency was removed from the executable package. | Architecture tests reject restoration of the legacy crate. Installed-host tests verify original Pi/OMP processes and the Claude plugin bridge. |

## Runtime contracts

Engine admission allows 256 pending plus active operations and 32 workers. The fixed-load regression held 31 prompt acknowledgments while 64 independent conversations completed, then held all 32 workers and verified producer backpressure at the queue boundary. One recorded local run completed the independent replies in 15.66 ms; see [performance](performance.md) and [raw results](benchmarks/engine-dispatch.json).

Same-session ordering, backend structural fences, event-gap recovery barriers, shared IM cooldowns and the task-board consumer cursor remain deliberate serialization boundaries. Ordinary remote calls execute in workers. Pending input state is taken as an owned value before remote calls, and adapter rate-state locks are released before HTTP I/O.

Service termination stops admission and gives running Engine workers one second to settle. Interrupted inbound work receives a durable uncertainty fence. Pending work has not claimed an event ID, completed work stays completed, and explicitly failed work remains retryable. Local preparation checkpoints state and detaches all live routes before sending IM requests. Notifications use bounded concurrency and one shared deadline, followed by bounded channel-task cleanup.

The slow-prompt regression originally failed because another conversation waited for an acknowledgment. The offline-notice regression failed because an unavailable IM send kept the service running. Both pass with the final implementation. Preparation, notification isolation, service-deadline and restart-deduplication tests verify the termination contracts.

## Initial verification (90b8cc4)

Verified on September 9, 2026, on macOS arm64 with Rust 1.95.0 and Node 26.8.1:

- `make check`: formatting, workspace Clippy with warnings denied, 713 passing Rust tests (11 existing exclusions), and 190 passing Node tests (four opt-in skips).
- `AGENTIX_TEST_NATIVE_HOSTS=1 node --test plugins/agentix-bridge/tests/native-host.test.mjs plugins/agentix-bridge/tests/claude-native.test.mjs plugins/agentix-bridge/tests/claude-delivery.test.mjs`: all 22 tests passed.
- `git diff --check`: no whitespace errors. The fixed-load result and reproduction command are stored with the performance documentation.

## Follow-up optimization acceptance

| Requirement | Implementation | Evidence |
| --- | --- | --- |
| Dispatch observability | Constant-size queue counters and timing gauges, periodic structured Engine/control output, persistent uncertain/rejected input counts, backend-specific timeout events. | Queue statistics tests; restart/count test in `state_and_render.rs`; bridge stalled-write and Codex timeout tracing tests; production runtime telemetry wiring. |
| Reduce snapshot overhead | Skip local routing snapshots when no work is pending or all workers are occupied. Resolve fresh routes whenever dispatch can resume. | `routing_is_needed_only_with_pending_work_and_available_workers`, existing transfer/opaque-action/replacement-event regressions. No speculative cache or CPU speedup claim. |
| Conversation admission fairness | A 16-input per-conversation quota within the existing 256-operation/32-worker bounds. Deduplicate pending IDs, release quota at retirement, and atomically reject overflow with coalesced durable feedback. | Production 300-input flood regression checks independent progress, pending duplicates, the accepted count, rejection replay fencing, and quota recovery. Existing fixed-load test retains global backpressure coverage. |
| Durable notification isolation | Runtime SQLite atomically stores the ingestion cursor and notification. Independent owned workers lease one head per conversation, retry with backoff, and fence stale acknowledgments. Task and overflow notices share bounded delivery capacity with alternating consumer priority. | `notification_outbox.rs` tests cover rollback, replay, restart, leases, ordering, retry and overflow coalescing. Production outbox test holds one IM send while existing and newly staged notifications reach other conversations. Task notification regressions cover paging and restart delivery. |

Task notifications have a 20-second send deadline and a 60-second recovery lease. Retry delays grow from 1 to 256 seconds. Delivery is at least once: a crash between remote acceptance and local acknowledgment can duplicate a notice. Overflow requests are rejected, not queued for automatic replay; their coalesced notice asks the user to resend after capacity becomes available. Global worker saturation and shared IM cooldowns remain intentional limits. See [performance and telemetry](performance.md#dispatch-telemetry-and-overload-handling) and [task board recovery](task-board.md).

The standard Rust suite retains 11 existing opt-in/helper exclusions: nine require an open Obsidian desktop vault, one exercises a large task database, and one is a subprocess helper exercised by its parent tests. Those desktop behaviors and the task-database implementation were not changed here. Native loader/delivery tests were run explicitly. External IM APIs and model output remain fixtures; no real IM messages or external model requests were sent.

The Claude smoke fixture now stops its isolated process group before removing temporary files, fixing a cleanup race found during verification. Performance evidence describes the stated fixtures and bounds; external host and IM latency remains subject to their own protocols and limits.

Final follow-up validation: `make check` passed formatting, strict workspace Clippy, 725 Rust tests (11 existing opt-in/helper exclusions), and 190 Node tests (four native opt-in skips). The native control fixtures now allow at most two simultaneous three-host stacks, avoiding unbounded child-process contention while retaining each test's concurrent backend requests and original watchdog deadlines. Both IM integration tests wait for actual notification API delivery after durable staging. `git diff --check` passed.

## Cross-platform CI follow-up

Shutdown uses a passive WAL checkpoint so an active reader or a cancelled background query cannot make checkpointing wait before offline notifications. Committed WAL records remain durable and are recovered on reopen; a regression holds a read snapshot while verifying checkpoint completion and persisted bindings.

Claude mailbox watchers resolve directory aliases before invoking libuv, including Windows short temporary paths. Protocol checks convert file URLs to native paths and generated contracts retain LF line endings. Cold-cache package installation tests allow two minutes per npm command and avoid shell descendants that can keep temporary directories locked after timeout.

## Slack transport addition

The [Slack review](slack-review.md) records the subsequent architecture findings, test-driven fixes, and measured performance improvements for the third IM adapter.

## Codex proxy ownership review (September 11, 2026)

The proxy belongs to the Codex connection layer. The executable assembles configuration and the adapter; Engine and IM handlers do not accept Codex sockets or infer terminal ownership. The audit covered startup, upstream reuse, authentication, transport transparency, registration, discovery and final-owner cleanup.

| Contract | Owner and evidence |
| --- | --- |
| Bind before upstream startup; reject occupied or aliased endpoints without deleting another owner's socket | ConnectionManager and proxy_address; CLI occupation tests and proxy endpoint regressions |
| Preserve a ready shared app-server across Agentix shutdown | UpstreamServer; subprocess survival/process-group test and runtime reuse test |
| Authenticate inbound WS before opening upstream; validate remote credentials in one layer | proxy_auth/proxy_handshake; real handshake tests for all seven options and failure ordering |
| Forward established Unix/WS frames without local Pong or added Close deadline | proxy_wire and paired relay tasks; control-frame, fragmentation, backpressure and closure tests |
| Adapt a single stdio client with bounded queues and cancellable I/O | proxy_stdio_io and stdio relay; blocked-stdout, EOF, shared-descriptor and subprocess-exit tests |
| Bind successful session replies to the actual connection, independent of cwd or PID | ClientRegistry; concurrent-client, request-ID, failed-response and broadcast regressions |
| Keep internal upstream subscriptions out of terminal discovery | CodexClient; production-runtime test with two same-directory clients and a disconnected but still loaded thread |
| Release background state when the last public client disappears | ClientTasks; final-owner regression for both direct and proxy clients, plus surviving-clone and frontend EOF checks |

The audit found a missing owner for the internal reader/reconnect and lifecycle-monitor tasks. A failing regression demonstrated that they retained upstream state after the public client was dropped. A connection-layer task guard now aborts both tasks on final-owner drop. Monitor clones deliberately omit the guard and proxy runtime to avoid retaining their own owner. Cancellation is processed asynchronously by Tokio; ready shared upstream processes remain detached.

The resulting ownership boundaries match the intended architecture. Transparency applies to established Unix/WS frames: authentication and HTTP routing are explicit proxy boundaries, and stdio is a JSONL adapter. Silent network partitions have no guaranteed detection deadline without a transport error; observation failure leaves forwarding intact but stops tracking that direction. These limitations are documented in the [guide](guide.md) and are not hidden by claiming app-server loaded threads prove terminal liveness. See [integration coverage](integration-coverage.md#codex-proxy-lifecycle) for executable checks and verification scope.
