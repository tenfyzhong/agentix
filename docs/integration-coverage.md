# Integration coverage

This map connects documented behavior to executable tests. It describes behavioral coverage, not a measured claim of 100% line or branch coverage. Tests use temporary databases, document trees, and local protocol services. The separately enabled desktop test uses an explicitly selected Obsidian vault.

## Running the checks

Use the pinned Rust 1.95.0 toolchain, Node.js 24+, and npm:

```sh
make check
```

This runs formatting, Clippy with warnings denied, workspace tests, and the plugin's Node tests. `make check` installs locked plugin dependencies. For a focused Cargo run, install them first:

```sh
npm ci --ignore-scripts --prefix plugins/taskix-manager
cargo test -p agentix-task -p taskix
node --test plugins/taskix-manager/tests/*.test.mjs
```

Cargo's `plugin_entrypoints_execute_the_compiled_taskix` test runs [integration.mjs](../plugins/taskix-manager/tests/integration.mjs) with the freshly compiled binary. It must not fall back to an installed taskix or the user's database. Set `TASKIX_TEST_HOOK_SHELL=fish` to additionally check hook commands through fish; Linux/macOS CI does this. Windows runs commands through `cmd.exe`.

## Tasks, projections, and host lifecycle

| Behavior | Executable coverage |
| --- | --- |
| Non-Git exact-root ownership, automatic registration/reuse, explicit overrides, same-name directories, Git/worktree/subdirectory identity, one discovery per context, symlink aliases, concurrent first visits, and projection-failure recovery | [Project resolution CLI tests](../crates/taskix/tests/support/project_resolution.rs), [taskix CLI tests](../crates/taskix/tests/cli.rs) |
| Indexed root/name query plans, unrelated malformed Project isolation, schema-13 migration, canonical root aliases, Unicode name collision/deletion, and existing session history/cancellation behavior | [Project resolution storage tests](../crates/agentix-task/tests/support/project_resolution.rs), [Incremental tests](../crates/agentix-task/tests/support/incremental.rs), [Inbox CLI tests](../crates/taskix/tests/support/inbox.rs) |
| First/repeated non-Git context latency at 1, 1,000 and 10,000 unrelated Projects | [Manual scaling benchmark](../crates/taskix/tests/support/project_resolution.rs), [measurement method and results](project-resolution-performance.md) |
| Claim → Plan → start → done; seven statuses, phase gates, failed transitions, revision checks, stale leases, expiry and resumption | [task_system.rs](../crates/agentix-task/tests/task_system.rs), [taskix CLI tests](../crates/taskix/tests/cli.rs) |
| Competing processes, concurrent projection writers, crash after commit, exact idempotent replay, database identity and path validation | [taskix CLI tests](../crates/taskix/tests/cli.rs), [task_system.rs](../crates/agentix-task/tests/task_system.rs) |
| Dependency validation, cross-Job prerequisites, one graph node for a shared prerequisite, seven colors, escaped labels, renamed/archived links, removal of legacy Dependencies prose | [job_graph.rs](../crates/agentix-task/tests/support/job_graph.rs), [CLI projection tests](../crates/taskix/tests/support/projections.rs) |
| Task notes before planning; managed tags/dependencies; local timestamps; freeform bodies and custom properties, CRLF frontmatter, quoted property keys, metadata-only Plan rejection; Board metadata; old meta/Task-list/Plan-path migrations | [tasknotes.rs](../crates/agentix-task/tests/support/tasknotes.rs), [CLI projection tests](../crates/taskix/tests/support/projections.rs) |
| Legacy Dashboard migration to Bases, read-only formulas, scoped filters, activity ordering, unchanged-file preservation, archive visibility, destination conflicts and partial-publication retries | [dashboard.rs](../crates/agentix-task/tests/support/dashboard.rs), [CLI projection tests](../crates/taskix/tests/support/projections.rs) |
| Job/Project deletion, active-lease and surviving-dependency rejection, cleanup retry after restart, unowned destination preservation after failed creation/renaming, owned files published before path registration, sequence retention, symlink boundaries | [deletion.rs](../crates/agentix-task/tests/support/deletion.rs), [taskix CLI tests](../crates/taskix/tests/cli.rs) |
| Codex/Claude manifest-selected hook commands using real CLI; Stop retains ownership without intake; interruption and shutdown release planning/executing leases; new-token recovery | [integration.mjs](../plugins/taskix-manager/tests/integration.mjs) |
| Manual Inbox intake after each Job: completion/idle events leave pending entries without Jobs, explicit claim creates one Job, and legacy `hook stop` cannot claim | [Inbox CLI tests](../crates/taskix/tests/support/inbox.rs), [inbox.test.mjs](../plugins/taskix-manager/tests/inbox.test.mjs), [integration.mjs](../plugins/taskix-manager/tests/integration.mjs) |
| Claude ordinary failures, missing flags, and string `"true"` do not release or renew; only boolean `true` requests interruption | [lifecycle.test.mjs](../plugins/taskix-manager/tests/lifecycle.test.mjs), [integration.mjs](../plugins/taskix-manager/tests/integration.mjs) |
| Pi/OMP real TypeScript entrypoints, automatic identity/token injection, Plan writes with full IDs and resolved Task prefixes, ambiguous-prefix rejection, errors, aborted processes, retry identity including Job/Project deletion | [runtime.test.mjs](../plugins/taskix-manager/tests/runtime.test.mjs), [integration.mjs](../plugins/taskix-manager/tests/integration.mjs) |
| Pi/OMP normal replies and automatic continuations retain executing leases; repeated SessionStart and old-session callbacks do not revoke current ownership | [lifecycle.test.mjs](../plugins/taskix-manager/tests/lifecycle.test.mjs), [integration.mjs](../plugins/taskix-manager/tests/integration.mjs) |
| Heartbeat timing, cancellation of in-flight renewal, queued ticks, failed cleanup retry, and new prompts waiting for cleanup | [lifecycle.test.mjs](../plugins/taskix-manager/tests/lifecycle.test.mjs), [runtime.test.mjs](../plugins/taskix-manager/tests/runtime.test.mjs), using controlled timers and runners |
| Marketplace resolution, hook discovery without duplicates, entrypoint selection, actual npm pack file list including TaskNotes guide/settings and local documentation links | [package.test.mjs](../plugins/taskix-manager/tests/package.test.mjs) |
| IM task actions, lease/revision/owner checks, reasons, notifications, delivery retry and durable event cursors | [Engine tests](../crates/agentix-core/tests/engine.rs) |
| IM dashboard → project Jobs → Job ↔ Task navigation; update ordering and Job status filters; adjacent entry controls, footer state actions and legacy list paging; session association and released work; owner/conversation/attachment scoping; ordered menus; project/Job/task pagination and archives; long fenced Markdown, reasons and titles; unavailable-document fallbacks | [IM browsing Engine tests](../crates/agentix-core/tests/support/task_board.rs) |
| Authored Task bodies and Job Goal/Notes in Obsidian, unchanged database/Plan hashes, and symlink escape rejection | [task_system.rs](../crates/agentix-task/tests/task_system.rs), `im_markdown_*` tests |
| Telegram/Feishu dashboard and detail callbacks, section ordering and split Telegram browse messages through actual transports → Engine → authored documents → MarkdownV2/card payloads, project/dashboard return links, attached `/board` and `/jobs`, unchanged task state | [task_browse.rs](../crates/agentix/tests/support/task_browse.rs), run by [channel_codex_e2e.rs](../crates/agentix/tests/channel_codex_e2e.rs) |
| Configured Telegram startup dashboard menu, exact primary order, alphabetical secondary commands and contextual labels | [application menu tests](../crates/agentix/src/main.rs), [Telegram API/menu tests](../crates/agentix-telegram/tests/telegram.rs), [Engine menu tests](../crates/agentix-core/tests/support/task_board.rs) |
| Telegram/Feishu task actions and reason replies through transport → Engine → SQLite → documents → channel notification | [channel_codex_e2e.rs](../crates/agentix/tests/channel_codex_e2e.rs), with local channel API services |
| Actual Dashboard columns/dates, archive filtering, native link navigation, TaskNotes Kanban columns/cards and task note recognition | [obsidian_smoke.rs](../crates/taskix/tests/obsidian_smoke.rs), opt-in desktop test |
| TaskNotes installation, settings/status merging, plugin enablement, backups, repeat runs, malformed bundles/configuration, path protection, download errors, rollback, automatic vault reload, CLI targeting, shutdown saves, reload failure/timeout and `--no-reload` | [obsidian_setup.rs](../crates/taskix/tests/obsidian_setup.rs), [installer tests](../crates/taskix/src/obsidian.rs); local release fixtures, HTTP server and native fake Obsidian CLI (no live desktop) |

The projection CLI tests cross argument parsing, a new process per command, SQLite persistence, and generated files. Library tests provide deterministic clocks and filesystem failure injection. Host integration tests run real taskix processes behind a minimal event API harness; controlled timer tests cover scheduling races without waiting a real minute.

## Other workspace boundaries

| Behavior | Executable coverage |
| --- | --- |
| Configuration, single selected channel, inactive credentials, Home paths, proxy validation, task-board opt-in | [configuration tests](../crates/agentix/tests/config.rs), [network tests](../crates/agentix/tests/network.rs) |
| CLI control transport, startup/shutdown, logging, argument errors, shell completions | [CLI tests](../crates/agentix/tests/cli.rs), [control tests](../crates/agentix/src/control.rs), [completions](../crates/agentix/tests/completions.rs) |
| Attachment, routing, draining sessions, queues, approval/input flows, retry, restart recovery, notifications and menus | [Engine tests](../crates/agentix-core/tests/engine.rs), [core behavior](../crates/agentix-core/tests/core_behavior.rs), [state/render tests](../crates/agentix-core/tests/state_and_render.rs) |
| Per-conversation FIFO, independent conversations and inbound/outbound progress, cancellation and backpressure | [message_center.rs](../crates/agentix-core/tests/message_center.rs), channel adapter suites |
| Codex RPC/event sequences, history/queue pagination and fallback, external writers, read-only attachment, process exit/resume, reconnects and background observation | [mock app-server integration](../crates/agentix-codex/tests/mock_app_server_integration.rs), [UDS client tests](../crates/agentix-codex/tests/uds_client.rs), [protocol tests](../crates/agentix-codex/tests/protocol.rs), [observed lifecycle tests](../crates/agentix-codex/src/client/lifecycle_tests.rs) |
| Slow IM response isolation, same-chat retry order, shared cooldown after cancellation, unrelated Telegram chat pacing, Feishu menu locking and concurrent token refresh | [Telegram tests](../crates/agentix-telegram/tests/telegram.rs), [pacing tests](../crates/agentix-telegram/src/rate_limit.rs), [Feishu tests](../crates/agentix-feishu/tests/feishu.rs) |
| Telegram owner/mention/claim rules, callbacks, API payloads, Markdown, FIFO retries and pacing | [Telegram tests](../crates/agentix-telegram/tests/telegram.rs) |
| Feishu long connection, official protobuf frames, callbacks, credentials, cards, reply lookup, token refresh and rate limits | [Feishu tests](../crates/agentix-feishu/tests/feishu.rs) |
| Feishu capacity-based cards: short views, lossless Unicode/escaped content, many panels, fenced code, growth/shrink, retry progress, action cleanup, send UUIDs and whole-group FIFO ordering | [Capacity tests](../crates/agentix-feishu/tests/feishu/capacity.rs), using the local Feishu HTTP fixture; card groups are tracked within an adapter lifetime, with no live-client or process-restart recovery proof |
| Full mocked Telegram/Feishu prompt → Engine → Codex → completed channel response | [channel_codex_e2e.rs](../crates/agentix/tests/channel_codex_e2e.rs) |
| rmux navigation, typed SDK requests, local socket exchange and process launch | [Engine tests](../crates/agentix-core/tests/engine.rs), [multiplexer tests](../crates/agentix-codex/src/multiplexer.rs) |
| Native release archives, checksums, version alignment and Homebrew formula transformation | [packaging tests](../crates/agentix/tests/packaging.rs) |

## OMP skill discovery

[Package tests](../plugins/taskix-manager/tests/package.test.mjs) verify the canonical plugin skill, absence of repository-root copies, and contained reference links in both the checkout and an installed npm tarball. The optional [native OMP test](../plugins/taskix-manager/tests/native-omp.test.mjs) starts an isolated RPC session and confirms `taskix-manager` appears in the host skill list and reads the full workflow and command reference through the native Read tool using `skill://` URIs. Run it with `AGENTIX_TEST_NATIVE_HOSTS=1`; it needs OMP and taskix on PATH, but sends no model requests and uses a temporary task database.

## Desktop and external acceptance

The normal suite needs no live account, model, host installation, or desktop. To run native Obsidian checks, follow [task board validation](task-board.md#validation): open the selected vault in the foreground, enable Bases and TaskNotes with the seven statuses, and supply `TASKIX_OBSIDIAN_VAULT` and an existing `TASKIX_OBSIDIAN_PARENT`. The test checks generated link destinations through Obsidian's native navigation API; it does not claim physical mouse or keyboard automation in every theme.

Host installer discovery/trust, real model behavior, host events on actual terminal interruption/exit, live IM credentials/permissions, and a live external rmux daemon require environment acceptance. A mock event test establishes what the adapter does when the event arrives; it cannot establish that every host version emits it. Force-kill and missed-hook recovery retain the lease-expiry fallback. Multi-machine or network-filesystem coordination is outside the supported concurrency model.

CI runs the full workspace suite on Linux/macOS. Windows checks the workspace and runs native TCP control plus task library/CLI/plugin tests. Timestamp tests verify the system-local offset on all platforms. Unix additionally tests process `TZ` overrides; Windows switches the runner system time zone through Tokyo (UTC+09:00), SA Pacific (UTC-05:00), and UTC, with explicit expected offsets and restoration in `finally`. Native Obsidian rendering is opt-in and is not run in CI. When adding a feature, extend the boundary tests and this map; do not describe an unexecuted live check as covered by its mock.

### Job verification and Obsidian status editing

| Behavior | Reusable tests |
| --- | --- |
| Automatic review readiness, rejection and resubmission, approval timestamps/events, Inbox gating, schema 8 migration | [Job review tests](../crates/agentix-task/tests/support/job_review.rs), [Inbox tests](../crates/agentix-task/tests/support/inbox.rs), [CLI review tests](../crates/taskix/tests/support/job_review.rs) |
| Snapshot identities, paths and lease omission | [Snapshot CLI tests](../crates/taskix/tests/support/obsidian_sync.rs) |
| Debouncing, ownership conflicts, revision fencing, timeout confirmation, rollback, newer edits, projection echoes and unload | [Sync engine tests](../plugins/taskix-manager/tests/obsidian-sync.test.mjs), [real CLI integration](../plugins/taskix-manager/tests/integration.mjs) |
| Dual Boards, scoped filters, pinned columns, Job properties and pastel status presets | [TaskNotes projection tests](../crates/agentix-task/tests/support/tasknotes.rs), [setup tests](../crates/taskix/tests/obsidian_setup.rs) |
| Embedded plugin installation, preserved settings, backups, malformed configuration and symlink rejection | [setup tests](../crates/taskix/tests/obsidian_setup.rs), [package tests](../plugins/taskix-manager/tests/package.test.mjs) |
| Desktop rendering and saved frontmatter edits with a temporary plugin instance and isolated database | [opt-in Obsidian smoke test](../crates/taskix/tests/obsidian_smoke.rs) |

Pi/OMP live bridge coverage includes shared control-listener CLI/native multiplexing, buffered event preservation, registration version checks, backend/session-root validation, duplicate-owner rejection, original-process controls, service restart and extension reconnection, plus persisted queue and host event tests in `plugins/agentix-bridge/tests`. Optional native host smoke tests load Pi and OMP without making model requests.

## Native bridge architecture and recovery

### Public interface audit after adding Pi, OMP, and Claude

The audit uses the actual `ControlRequest`, `SessionOperation`, `SessionCommand`,
and `AgentCommand` enums and the bridge/MCP request handlers as its inventory.
Coverage means each supported interface has behavioral checks at its owning
boundary, with cross-layer tests for routing and host differences. It does not
mean every possible input or every host/IM/operation combination is enumerated.

The previously missing boundary was the complete native control path. The new
[native control suite](../crates/agentix/src/native_control_tests.rs) runs the
production Unix listener, control handler, `SessionOperations`, `AgentRegistry`,
`BridgeAdapter`, and `BridgeHub`, with all three production Node plugin runtimes
connected simultaneously. It is compiled into the binary test target so it can
exercise the real private assembly functions. Pi/OMP's host API and Claude's MCP
client/hooks are local subprocess fixtures; the listener, routing, bridge,
history projection, queue, and Claude persistence implementations are not mocked.
The separate [CLI process suite](../crates/agentix/tests/cli.rs) verifies argument
parsing, request encoding, output, and process exit status.

| Public interface | Behavioral evidence |
| --- | --- |
| `client sessions` / control `sessions` | All three native hosts on one listener; qualified IDs, Pi/OMP's identical native ID, one-item pagination without duplicates; CLI JSON output; Codex loaded-session pagination in its mock app-server suite |
| `client send` / session `send` | Original plugin receives the exact prompt and returns a turn ID; completed history contains the expected user and assistant text; empty, busy, missing, and offline sessions fail; CLI checks all four backend IDs |
| `client send --turn` / steering | CLI preserves the expected turn; Pi/OMP accept an active turn and reject idle steering; Claude rejects the unsupported operation |
| `client stop` / session `stop` | Pi/OMP interrupt the original turn and pause queued prompts without killing the host; Claude rejects; generic Engine tests check owner-scoped stop actions |
| `client history` / session `history` | All three native hosts preserve turn IDs through older/newer cursor traversal; malformed cursors fail; CLI preserves opaque cursors and limits; Codex covers paginated and fallback history |
| `client command` / session `command` | Pi/OMP exercise status, model listing/selection, reasoning read/write, rename, compact, skills, and diff; Claude exercises status and rejects the other operations |
| Unsupported native commands | Fork, fast, clear, exit, plan, goal, review, and MCP return capability errors; they do not become model prompts. Codex's `every_attached_session_command_runs_against_the_mock_server` covers these supported Codex commands |
| `client call` / control `call` | CLI raw JSON forwarding and mock Codex RPCs; a native-only registry returns the explicit Codex-only error |
| `client claim` / control `claim` | CLI output, real handler code generation and invalid TTL; existing owner-claim tests cover expiry, consumption, and owner persistence for both IM channels |
| `doctor`, `serve`, completions | Native doctor checks Pi/OMP/Claude without a listener; Codex mock handshake; service lifecycle tests cover offline saved bindings, startup, shutdown, and socket cleanup; shell completion/config suites cover the CLI setup boundary |
| Engine dispatch isolation | Production runtime tests hold a prompt or working-card send while another conversation continues, preserve same-conversation order, and bound shutdown while fencing uncertain inputs across restart. Shutdown tests also separate local preparation from IM effects and verify independent notices within a shared deadline. Engine routing tests cover consecutive transfers, displaced chats, opaque attach buttons, and early replacement events; both IM end-to-end suites use the production runtime |
| Control dispatch isolation | A real Pi plugin withholds prompt acknowledgment while OMP, catalog, same-session history and stop complete; handler cancellation finishes without releasing the host gate. Resource-queue tests cover bounds, FIFO, backend-exclusive operations and native aliases |
| Control framing and errors | Unix/TCP round trips, loopback restriction, socket permissions, malformed and oversized requests, shared native/CLI listener; CLI errors produce nonzero exit and no success JSON |

The queue is exposed through IM and `QueuedPromptPort`, not a separate CLI session
operation. Its native control-stack tests verify duplicate request IDs, pending
items, interruption/pause, explicit resume, completion, and backend isolation.
Claude supports inspecting/clearing delivery uncertainty, but does not advertise
Pi/OMP FIFO submission.

| IM interface group | Behavioral evidence |
| --- | --- |
| `/sessions [backend]`, `/attach`, `/current`, `/detach`, `/help` | Engine picker/filter/transfer/current/help tests; both mocked IM transports attach to each production native plugin and receive its reply |
| Prompt, `/steer`, `/stop`, stop buttons | Core turn/owner/action tests plus native control and runtime tests; Codex approval and input dialogs use its stateful mock app-server |
| Card write ordering | Engine module regressions cover same-card serialization/coalescing, stale query revisions, duplicate admission, late remote commits after cancellation/timeout, replacement aliases across reload and callbacks, independent-card progress, action disabling composed with pending final content, cancelled local waiters, separate admission/remote deadlines, definite rejection recovery, binding changes while waiting and uncertain replacement sends. Adapter tests verify progress markers, Slack cooldowns longer than the wire deadline, malformed response uncertainty, and Feishu disable retry cache retention. Guarantees cover Engine-managed cards within one runtime and reloads; live-provider timing and remote requests surviving process restart remain outside this proof. |
| Background completion notices | Engine tests cover complete/partial cached output, stalled history, bounded retry, exact loading-card updates, overlapping turns, recipient failures, deduplication, queue saturation, reload and generation fencing. Production runtime tests cover same-session event progress and cancellation with retained Engine snapshots; the release admission benchmark and its limits are documented in [Background completions](background-completions.md). Real IM/provider timing remains manual. |
| `/last` | Engine tests cover contextual menu/help, latest history, live content and Stop transfer across Telegram/Feishu/Slack abstractions, continued updates, cold process output, repeated requests, background isolation, read-only refresh, empty history, and unchanged pagination; real-client layout remains manual |
| `/history`, older/newer buttons | Engine cursor/render tests plus native control pagination and native session projection tests |
| `/queue`, `/queue resume`, `/queue clear` | Core queue/control and scoped-button tests, native queue-stack tests, runtime persistence tests; both IM transports query every native backend's empty queue |
| Session commands and `/cancel` | Core command gating, model/reasoning choice, rename input, plan/custom input and cancellation tests; native control command matrix; both IM transports query `/status` and reject `/fork` for Pi/OMP/Claude |
| `/rmux [backend]` and workspace actions | Core backend-selection/navigation/mutation tests, registry qualification tests, typed rmux SDK/local-socket tests; no external rmux daemon is required by the normal suite |
| `/dashboard`, `/board`, `/jobs`, `/tasks`, `/task`, `/inboxes`, `/inbox` and task actions | Task browse/action round trips through both mocked IM APIs, SQLite and document projection; task-board and Inbox suites above cover state/ownership rules |
| Native callbacks and transport policy | Telegram/Feishu suites cover owner/mention/claim policy, action acknowledgement, payloads, retry/pacing, and Feishu long-connection frames; Engine tests cover stale/foreign action rejection |

| Native/plugin interface | Behavioral evidence |
| --- | --- |
| bridge `register`, `inspect`, reconnect and events | Shared production listener and hub suites: version/root/duplicate ownership, buffered events, lifecycle, foreign identity, cancellation, connection recovery and host preservation |
| bridge `info`, `snapshot`, `history` | Metadata listing, shared schema/DTO contracts, bounded history and stable cursors; all three hosts cross the production Rust/Node boundary |
| bridge `prompt`, `steer`, `stop`, `command` | Native control suite plus runtime tests for receipts, invalid requests, host errors, session switches, and busy/unsupported operations |
| bridge `queue`, `queue_state`, `queue_resume`, `queue_clear` | Runtime FIFO/replay/atomic persistence and native registry tests; Claude uncertainty/clear tests explicitly verify that clearing never resends a prompt |
| MCP initialize, tools/list, tools/call, Channel notification | Production Claude MCP subprocess tests in both modes; exact tool sets; valid acknowledge/reply; unknown tools and invented receipt IDs fail without turn/message events; reconnect retains identity/history |
| Claude hooks and rmux delivery | Mailbox process isolation and failed-event retention; session tests for foreign hooks, completion and receipt recovery; delivery tests for matching acknowledgements, preflight failures, partial-paste uncertainty and no automatic resend |

External Telegram/Feishu APIs and model behavior remain mocked. The normal tests
need no authenticated host account or external model/IM request. Optional native
loader and controlled-rmux tests validate installed host integration separately;
they do not establish live Channel availability or live account delivery. These
boundaries are intentional and should remain explicit when extending the suite.

Verification on September 9, 2026: `make check` passed with 689 Rust tests
(11 ignored) and 190 Node tests (4 opt-in skips). After the final history/doctor
matrix additions, the complete five-test native control suite, eight-test CLI
suite, two-mode MCP suite, and final all-target/all-feature Agentix Clippy check
passed. The optional native loader/delivery command below also passed all 22
tests. No production behavior change was needed for the gaps found by this audit.

| Behavior | Executable coverage |
| --- | --- |
| Shared control listener serves ordinary CLI requests and persistent native registration; a generic session operation traverses `SessionOperations` to the original connection | [control.rs](../crates/agentix/src/control.rs), [CLI tests](../crates/agentix/tests/cli.rs) |
| Independent native ownership, duplicate/root/version rejection, reconnect, detach without host termination, lifecycle events, bounded concurrent metadata listing and rejection of foreign identity | [bridge integration](../crates/agentix-bridge/tests/bridge.rs) |
| Invalid/misrouted events, cancellation cleanup, oversized outbound frames and write-inclusive deadlines | [connection unit tests](../crates/agentix-bridge/src/bridge.rs) |
| Shared schema, generated Rust/TypeScript DTO freshness, required nullable fields and explicit domain conversion | [Node contract tests](../plugins/agentix-bridge/tests/protocol.test.mjs), [Rust protocol](../crates/agentix-bridge/src/protocol.rs) |
| FIFO, incremental journal replay, legacy checkpoints, duplicate records, immutable snapshots, uncertain reload and failed persistence atomicity | [queue tests](../plugins/agentix-bridge/tests/queue.test.mjs), [runtime integration](../plugins/agentix-bridge/tests/runtime.test.mjs) |
| Original-session event/command handling, host switch during pending operations, idempotent failed delivery, pause/clear/resume, metadata without history, and stop/read progress during slow commands | [runtime integration](../plugins/agentix-bridge/tests/runtime.test.mjs) |
| Stable history identity, branch pagination, restored completion reconciliation, activity timestamps and cache invalidation | [session tests](../plugins/agentix-bridge/tests/session.test.mjs) |
| Both Telegram and Feishu reach Pi, OMP, and the Claude MCP plugin through original-process fixtures | [channel end-to-end tests](../crates/agentix/tests/channel_codex_e2e.rs) |
| Claude rmux/Channel MCP capability separation, hook replies, acknowledgement and reconnect | [MCP integration](../plugins/agentix-bridge/tests/claude-server.test.mjs), [Rust contract](../crates/agentix-bridge/tests/bridge.rs) |
| Claude incremental persistence, legacy replay, partial tails, deduplication and linear serialization growth | [store tests](../plugins/agentix-bridge/tests/claude-store.test.mjs) |
| Claude failed persistence, receipt timeout, completion/clear rollback and recovery | [session tests](../plugins/agentix-bridge/tests/claude.test.mjs) |
| Hook mailbox process isolation, failed-event retention, controlled notification coalescing and shutdown; real MCP consumption with native notifications or missed-notification polling | [mailbox tests](../plugins/agentix-bridge/tests/claude-mailbox.test.mjs), [MCP server tests](../plugins/agentix-bridge/tests/claude-server.test.mjs) |
| Per-frame size limits, fragmented UTF-8 and coalesced frames | [framing tests](../plugins/agentix-bridge/tests/framing.test.mjs), [transport tests](../plugins/agentix-bridge/tests/transport.test.mjs) |
| Stalled registration leaves other sessions responsive; duplicate reservation and cancellation cleanup | [hub unit tests](../crates/agentix-bridge/src/bridge_hub.rs) |
| Offline saved native binding still permits Unix control listener startup | [service lifecycle tests](../crates/agentix/src/main.rs), [core recovery tests](../crates/agentix-core/tests/engine.rs) |
| Installed Claude plugin uses original host PID; real rmux clears multiline drafts before submission | [native plugin smoke test](../plugins/agentix-bridge/tests/claude-native.test.mjs), [delivery tests](../plugins/agentix-bridge/tests/claude-delivery.test.mjs) |
| Native package loading through installed Pi and OMP public extension APIs | [optional loader smoke tests](../plugins/agentix-bridge/tests/native-host.test.mjs) |

On September 9, 2026, `make check` passed on macOS arm64 with Rust 1.98.0 and Node 26.8.1. The final focused shared-control test and Clippy check also passed after extending that test's operation-service coverage. `rustup run 1.95.0 cargo check --workspace --all-features --target-dir /tmp/agentix-msrv-target` passed for the minimum supported Rust version.

The workspace Rust run passed 675 tests with 11 ignored entries (the isolated daemon helper, opt-in large-database acceptance, and desktop Obsidian checks). The default combined Node run passed 149 tests and skipped the two opt-in native loader tests. Both loaders passed separately with Pi 0.84.4 and OMP 17.3.7:

```sh
AGENTIX_TEST_NATIVE_HOSTS=1 node --test plugins/agentix-bridge/tests/native-host.test.mjs
```

These runs use temporary local protocol services and isolated host configuration. They do not test paid model requests, real Telegram/Feishu account delivery, native third-party approval dialogs, or a Windows native bridge. The opt-in large-database and desktop Obsidian checks were not run; the ignored daemon helper is exercised indirectly by its parent tests. Performance workloads and their limitations are recorded in [performance](performance.md); they are not substitutes for the integration suite.


### Verification after the Claude architecture review

The review run on September 9, 2026 used macOS arm64, Rust 1.95.0, and Node 26.8.1. `make check` passed: formatting, workspace Clippy with warnings denied, 684 Rust tests (11 ignored), and 190 Node tests (4 opt-in skips). The two IM bridge tests were then extended to include Claude and passed for both Telegram and Feishu. Packaging tests and protocol/SDK generation checks passed; both repository and standalone plugin package manifests include the shared frame decoder and Claude journal store.

The opt-in command below passed all 22 tests with Pi 0.85.1, OMP 18.1.15, Claude Code 2.1.236, and rmux 0.10.0:

```sh
AGENTIX_TEST_NATIVE_HOSTS=1 node --test plugins/agentix-bridge/tests/native-host.test.mjs plugins/agentix-bridge/tests/claude-native.test.mjs plugins/agentix-bridge/tests/claude-delivery.test.mjs
```

This includes original-process loader checks and the controlled real-rmux draft-clearing test. Channel protocol delivery is tested through the production MCP plugin with a local Claude-client fixture, not a live authenticated model. No real IM messages or external model requests were used. See the [review findings and boundaries](native-bridge-review.md).

### Verification after runtime layering and dispatch changes

The follow-up architecture acceptance run passed `make check`: formatting, strict workspace Clippy, 725 Rust tests (11 existing opt-in/helper exclusions), and 190 Node tests (four native opt-in skips). New coverage includes queue statistics, snapshot admission guards, per-conversation overload rejection and recovery, durable notification cursor/lease/retry invariants, and independent notification delivery through the production runtime. Existing concurrency, global limits, dynamic bindings, uncertainty fencing and bounded offline notifications remain covered. Both IM end-to-end suites wait for actual notification delivery through that same runtime. See [Architecture optimization review](architecture-review.md) for current evidence and the earlier 22-test explicit native-host validation.

## Slack coverage

- [Startup CLI tests](../crates/agentix-slack/tests/startup.rs) and [manifest tests](../crates/agentix-slack/tests/manifest.rs): fetch/merge/install/verify, unchanged skips, retained settings, native command routing, timeouts, and failure isolation. CLI process fixtures run on Unix; manifest and event tests run on all platforms.

- [Protocol and API tests](../crates/agentix-slack/tests/adapter.rs): bot/app credentials, send/edit/disable, thread destination, rate-limit retry, Socket Mode ACK/reconnect/shutdown, private one-time owner claims.
- [Event tests](../crates/agentix-slack/tests/events.rs): workspace and owner rejection, bot filtering, mentions, thread isolation, edit versions, action references, and slash commands.
- [Render tests](../crates/agentix-slack/tests/render.rs): Unicode limits, escaped mentions, code text, and action bounds.
- [Codex end-to-end test](../crates/agentix/tests/support/slack_e2e.rs): real local WebSocket and HTTP transports through Engine and Codex RPC, command approval callback, and final streamed update.
- [Core state tests](../crates/agentix-core/tests/state_and_render.rs): SQLite restart recovery, thread isolation, and cross-platform event deduplication. Critical interaction/Stop regressions also run for Slack in `engine.rs`.

These fixtures require no Slack credentials. They do not prove workspace installation or permissions; follow [Slack setup](slack.md) for a live smoke test.

## Codex proxy lifecycle

| Behavior | Executable coverage |
| --- | --- |
| Actual `serve` exits nonzero and logs ERROR for occupied Unix/WS endpoints; preserves unidentified live sockets and ordinary files; does not launch upstream | `serve_exits_and_logs_when_proxy_endpoint_is_occupied` in [CLI tests](../crates/agentix/tests/cli.rs) |
| Abandoned frontend recovery, verified Codex listener reclamation, launcher descendants, cancelled startup, and external child kill cleanup | `proxy_recovers_abandoned_socket_but_preserves_live_listener_and_symlink`, `proxy_reclaims_confirmed_codex_app_server_listener`, `wrapped_upstream_shutdown_reaps_descendant_and_removes_socket`, `cancelled_upstream_startup_removes_unready_child_socket`, and `owned_upstream_external_kill_removes_socket` in [proxy transport tests](../crates/agentix-codex/tests/proxy_transport.rs) |
| Real `serve` recovers a stale frontend before launching upstream and cleans it after a startup SIGTERM; shutdown interrupts a stalled initial identity lookup with owned client clones | `serve_recovers_stale_proxy_before_starting_upstream` in [CLI tests](../crates/agentix/tests/cli.rs) and `startup_signal_cancels_identity_lookup_and_stops_owned_upstream` in [reload tests](../crates/agentix/src/reload_tests.rs) |
| A stalled channel shutdown does not postpone owned Codex process/socket cleanup | `service_stop_reaps_codex_while_channel_shutdown_is_stalled` in [reload tests](../crates/agentix/src/reload_tests.rs); isolated subprocess coverage, not a live Homebrew restart |
| Unix/WS forwarding, WS upstream, stdio JSON lines, independent request IDs, unchanged streaming and approval frames, downstream/upstream disconnect cleanup, socket inode ownership, owned upstream termination/reaping, adopted upstream preservation, cancelled/concurrent shutdown, forced termination and replacement socket protection | [proxy transport tests](../crates/agentix-codex/tests/proxy_transport.rs) |
| Successful start/resume/fork binding, unsubscribe, multiple owners, ignored broadcasts and failed requests, PID metadata | [proxy registry tests](../crates/agentix-codex/tests/proxy_registry.rs) |
| Registry-backed session listing and stale attachment rejection | [proxy transport tests](../crates/agentix-codex/tests/proxy_transport.rs) |
| Accepting another client before peer identification finishes | [proxy unit tests](../crates/agentix-codex/src/proxy.rs) |

These tests use local transports and reusable subprocess fixtures. Authentication is exercised through real WS handshakes on local test listeners; these tests do not establish WAN/TLS deployment behavior or live IM delivery. Remote PID discovery is intentionally unsupported. The retained [session proxy example](../crates/agentix-codex/examples/session_proxy.rs) supports optional checks with a real Codex CLI. The ordinary test suite requires no model request.

Codex proxy authentication tests exercise real WebSocket upgrades for capability token files/digests, missing and invalid credentials, Origin rejection, duplicate Authorization headers, JWT signatures/expiry/not-before/issuer/audience/algorithm, and mandatory authentication outside loopback. Registry tests cover unknown-PID connections and multiple owners of one session. An idle connection test verifies that sessions survive without heartbeats and that only the upstream supplies Pong. CLI/configuration tests cover the seven authentication flags and nested proxy options.

Transparent control-frame coverage verifies both Unix/WS frontends, original masking and payload bytes, no early/duplicate Pong, upstream EOF cleanup, handshake-prefetched frames in both directions, and fragmented text with interleaved Ping frames.

Observer-failure tests verify bidirectional raw forwarding after invalid UTF-8 and buffer release after an observation size limit. Close-handshake tests cover either peer initiating, a delayed fragmented reply, no premature EOF, and both transports reaching EOF with registration cleanup after the two Close frames are forwarded.

Proxy failure-path coverage includes deterministic 64-byte-buffer backpressure with reverse Ping and EOF, injected EMFILE preserving existing clients and accepting new ones after recovery, EOF after the first Close, delayed upstream upgrade and subprotocol preservation, original HTTP rejection headers and fragmented body, unavailable-upstream 502, and handshake-timeout 504. Authentication tests assert that Authorization never reaches upstream and that valid credentials with an unavailable upstream receive 502 rather than a premature 101.

Proxy boundary regressions also cover IPv6 listener/upstream connections, stdio reverse requests and Pong handling under stdout backpressure followed by input EOF cleanup, stalled successful handshake response writes, and HTTP keep-alive rejections with zero/nonzero Content-Length and chunked trailers. HTTP unit tests cover fragmented chunks, malformed/truncated bodies, oversized lines, and conflicting Content-Length values.

Endpoint regressions in [proxy_endpoint_regressions.rs](../crates/agentix-codex/tests/proxy_endpoint_regressions.rs) cover internal IPv6 initialization, IPv6 loopback startup classification, explicit default WS port handling, parent-directory aliases with missing directories, dangling upstream symlinks, cleanup on rejection, and distinct sockets within aliased directories. These tests run by default.

Final lifecycle coverage in [proxy_lifecycle_regressions.rs](../crates/agentix-codex/tests/proxy_lifecycle_regressions.rs) proves that stdio Close clears registrations and releases the upstream socket before blocked stdout drains, preserves the queued response, and lets a subprocess exit with stdin still open and idle. [Address checks](../crates/agentix-codex/src/proxy_address.rs) cover URL/DNS aliases, wildcard listeners, IPv4-mapped IPv6, distinct ports, and actual-peer rejection before the upstream handshake. Standard-I/O unit tests cover redirected files and restoration of shared descriptor flags. All former known-defect exclusions have been removed; the stdio subprocess fixture is invoked by its parent test.

The September 11, 2026 follow-up passed all 153 default Codex tests (four helper/manual-benchmark exclusions), 48 CLI/configuration tests, and the main-program proxy test on macOS. Strict Clippy for `agentix-codex` and `agentix` across all targets, formatting, and whitespace checks passed. The audit covered authentication and HTTP upgrades, raw frame forwarding, backpressure and closure, connection-owned registrations, endpoint identity and socket cleanup, stdio cancellation, and connection-layer ownership. No unresolved defect remained from this audit. This run used local protocol fixtures; it did not repeat WAN, live model, Linux runtime, or performance measurements.

A subsequent focused review added standard-I/O tests for bidirectional traffic over a shared socket descriptor, reverse drop order, and retaining an originally nonblocking descriptor. All four standard-I/O tests and 38 endpoint/lifecycle/registry/transport tests passed on macOS, along with strict Codex Clippy. No production logic changed and no new defect was confirmed in this review.

The subsequent performance pass ran all 157 default Codex tests and 48 CLI/configuration tests successfully. It added normal regressions for method-first notification rejection (escaped keys, reordered fields, malformed frames and pending ownership responses) and single-line/multiline stdio JSON output. The release-only transport matrix, paced RPC comparison, stdio comparison, accept-delay regression, large-message overhead regression, and observation regression also passed. Strict Clippy across both crates' targets, formatting and whitespace checks passed. See [performance results and reproduction commands](performance.md#codex-proxy-transport-and-observation); timing tests remain opt-in because host load affects them.

The connection-ownership architecture review adds two regressions in [client lifecycle tests](../crates/agentix-codex/src/client/lifecycle_tests.rs). The first reproduces retained reader/monitor state after the final public client is dropped, and verifies cancellation for both direct and proxied clients while a surviving clone remains usable. The second composes the production connection manager, Unix/WS listeners, mock upstream, registry, and public session API: two same-directory clients reuse RPC IDs, B disconnects while its thread remains loaded upstream, only A stays visible, stale B attachment fails, and dropping the runtime releases frontend connections/registrations while the adopted external upstream remains reusable. Neither test injects ownership directly into the registry. Service lifecycle regressions additionally launch an isolated mock app-server and verify all supported shutdown signals, process reaping, same-path restart, and deferred Codex shutdown after reload while public client clones remain alive.

Final verification after the ownership fix passed the full repository `make check` on macOS: workspace formatting, Clippy with all targets/features and warnings denied, the complete default Rust workspace unit/integration/doc-test suite, and 222 Node tests (five opt-in skips). The run includes all proxy Unix/WS/stdio suites, CLI/configuration tests, and Telegram/Feishu/Slack integration fixtures. Manual timing benchmarks and environment-dependent native/live tests remain opt-in; this ownership-only fix did not repeat performance measurements, WAN/TLS deployment, live model requests, or Linux runtime checks. `git diff --check` also passed.

The opt-in [native CLI benchmark](../crates/agentix-codex/tests/native_cli_performance.rs) additionally launches the installed Codex 0.154.0 TUI through a reusable PTY driver, completes a warmup turn, and verifies immediate and paced mock replies both directly and through the production Unix proxy. Forty release-mode samples passed. Supporting mock changes cover TUI configuration/account/bootstrap responses, UUID thread IDs and empty previews as strings. The 53 default Codex unit tests and 41 mock integration tests passed, and strict all-target Codex Clippy, formatting and whitespace checks passed. This supplement changes test support and benchmark documentation only. See [native measurements](performance.md#native-codex-cli-against-a-mock-app-server) for scope and reproduction.

The rmux pane lifecycle also has an opt-in real-daemon regression test: `cargo test -p agentix-rmux live_rmux -- --ignored`. It uses an isolated socket and checks foreground process detection, Ctrl-C, and shell input after agent exit. New session/window/split coverage verifies working directories and Ctrl-D closure; with `AGENTIX_LOGIN_SHELL=fish`, it also verifies isolated fish login and interactive configuration loading. The normal SDK wire tests verify explicit login-shell commands for new sessions/windows/splits and execute the agent launch command with successful and unsuccessful child exits and literal arguments.

Codex workspace launch regressions in [workspace_launch.rs](../crates/agentix-codex/tests/workspace_launch.rs) verify that rmux and tmux launches pass an explicit absolute `--cd` for existing panes, new sessions, windows, and splits, including paths with spaces, quotes, and Unicode. Empty panes receive no agent command. These tests use the local mock app-server and assert the multiplexer launch boundary; they do not launch a real Codex TUI or contact a model.

Native tmux lifecycle coverage runs with `AGENTIX_TEST_TMUX=1 cargo test -p agentix-tmux`. It verifies normal and unsuccessful agent exits, Ctrl-C with foreground process detection, and subsequent shell input in the requested working directory. It also checks that new sessions/windows/splits override the server default command with the user's login shell and close on Ctrl-D, even when global pane retention is enabled. The tmux CI jobs enable these tests with the account shell and fish on Linux and macOS.

## Native `/new` coverage

- Core regression tests cover the exit/create gap, ordered delivery after binding, foreign-client rejection, manual detach/attach cancellation, timeout and late arrival, restart with a temporarily unavailable replacement, and no replay of uncertain sends.
- Storage tests cover atomic binding/FIFO transitions, epoch and revision fences, restart persistence, duplicate queue IDs, and bot-identity cleanup.
- Pi/OMP bridge tests cover native lifecycle registration with stable client identity. Pi's command-context test checks stop-before-new without a model prompt. A Node subprocess and real Unix IPC exercise Rust registration and replacement events.
- Codex native command tests in `workspace_launch.rs` cover metadata-only paged turn inspection (one turn with unloaded items, no input enrichment), paged inspection when embedded history is unsupported, legacy history fallback, empty/unmaterialized sessions with live idle confirmation when history is unavailable, interrupt-before-switch for active turns, and preserved history errors without terminal mutation.
- Codex registry tests cover coalesced notifications, fork exclusion, both start/unsubscribe orders, closed-connection replacement fencing, and ordinary unsubscribe/exit without switch events. The mock app-server integration verifies delayed native replacement before exit cleanup and ordinary exit without a waiting notice or persisted timeout, and exit notification on socket closure while the client process is still alive. Terminal policy tests reject shell panes, drafts, dialogs, and copy/dead modes. Claude tests cover asynchronous acceptance and terminal failure.

These are deterministic protocol/engine tests and isolated fixtures, not live Feishu/Slack or real model-provider acceptance. Real Codex/OMP/Claude input layouts, customized key bindings, competing terminal typing, extension cancellation by other plugins, and real-client restart timing need manual verification. Terminal checks are best effort and cannot atomically lock a human-operated terminal.

### Handoff acceptance matrix

| Behavior | Automated evidence |
| --- | --- |
| Exit/create gap and FIFO turn ordering | `engine.rs`: `new_session_waits_through_exit_and_drains_gap_messages_in_order` |
| Repeated requests and replacement events | `engine.rs`: `new_session_duplicate_requests_and_events_do_not_repeat_delivery` |
| Queue capacity, overflow feedback, clear without cancelling the pending handoff | `engine.rs`: `new_session_queue_limit_and_clear_preserve_the_pending_handoff` |
| Late failure after successful replacement | `engine.rs`: `new_session_failure_from_old_client_cannot_pause_committed_replacement` |
| Client identity isolation and explicit detach | `engine.rs`: `native_new_session_follows_only_matching_client_and_honors_manual_detach` |
| Explicit attach with retained old messages | `engine.rs`: `new_session_manual_attach_retains_old_queue_without_capturing_new_prompts` |
| Timeout, late arrival, queue inspection and discard | `engine.rs`: `new_session_timeout_clears_flow_and_late_replacement_cannot_attach` |
| Restart, temporary attach failure, uncertain delivery and blocked replay | `engine.rs`: `new_session_retries_attachment_after_restart_without_replaying_uncertain_prompt` |
| Destination already owned by another conversation | `state_and_render.rs`: `native_session_handoff_cannot_displace_another_conversation` |
| Atomic binding/queue commit and stale worker rejection | `state_and_render.rs`: `native_session_binding_and_queue_commit_atomically_with_epoch_fence`, `session_switch_survives_restart_and_rejects_stale_queue_updates` |
| Original terminal, drafts, copy mode, dead pane, foreground changes before Enter | `plugins/agentix-bridge/tests/new-session.test.mjs`; Codex prompt validation in `agentix-multiplexer` |
| Stable registration identity across real local IPC | `agentix-bridge/tests/bridge.rs`: `native_new_crosses_ipc_with_stable_identity_and_replacement_registration` |

The core test paths above are under `crates/agentix-core/tests/`. This matrix describes behavioral coverage, not a line-coverage percentage. Native terminal tests inject terminal state and failures; they do not certify every host version's actual screen layout. Real-client checks should exercise idle and active turns, two simultaneous clients in the same directory, manual native new, and service restart during the gap before rollout.

### Terminal draft confirmation coverage

Core `terminal_draft_*` tests cover prompt and `/new`, confirmation, cancellation, repeated buttons, changed drafts, attachment changes, and durable handoff queues across restart, confirmation and `/cancel`. Claude delivery tests cover multiline capture, refusal to auto-clear, conditional clearing, changed snapshots, empty inputs and clearing failures. The opt-in native rmux/tmux tests exercise these steps through real isolated panes using a deterministic terminal host; `AGENTIX_TEST_NATIVE_HOSTS=1 cargo test -p agentix-multiplexer --test native_input` covers Codex multiline capture and conditional clearing on both drivers, including their different treatment of background-only rows. Codex parser tests cover complete multiline boxes, a cursor at the beginning, disabled/dead/copy-mode panes, clipped boxes, and styled borderless composers with dim placeholders. The layout handling follows the upstream [Codex composer](https://github.com/openai/codex/blob/main/codex-rs/tui/src/bottom_pane/chat_composer.rs); customized themes, hidden paste payloads and host viewport clipping remain manual verification boundaries.

Codex default-background composer regressions reproduce the v0.154.0 rmux screen layout, including dim placeholders, multiline drafts, status footers and Vim mode checks. The isolated native input test also submits `/new` after confirmation on both colored and default-background layouts in tmux/rmux. Core tests verify terminal inspection errors are returned to IM.


Codex submission regressions model its unbracketed input burst: an early Enter inserts a newline. The terminal sender waits for two stable rendered command observations before submitting. Core retry coverage verifies that a timed-out switch can send a new command after draft confirmation with the old queue cancelled, and that an active switch preserves a newly typed draft.

The opt-in real TUI test in `native_input.rs` accepts `AGENTIX_TEST_CODEX_BINARY` and `AGENTIX_TEST_CODEX_ENDPOINT`, then runs `cargo test -p agentix-multiplexer --test native_input real_codex`. It launches isolated tmux/rmux panes, trusts only their temporary test directory, submits `/new` from an empty composer and after conditional draft clearing, and verifies the composer returns to empty. Both drivers passed with Codex 0.154.0 on macOS. This test uses a supplied local app-server; it submits no model prompt and does not verify live Feishu delivery or IM reattachment.

The Codex registry excludes ephemeral helper threads from ownership and native handoff candidates. Regression tests cover helpers between unsubscribe/start in either order, with ephemeral metadata on the request, response, or both. The opt-in `cargo test -p agentix-codex --test native_handoff` uses the same real-Codex environment variables to verify production proxy events, namespaced session IDs, and Engine conversation reattachment through an isolated local channel sink on both tmux and rmux. It does not send live IM messages.

Terminal detection tests cover rmux with a tmux compatibility alias pointing to the same socket, pane and verified process root. Equivalent aliases count as one target; distinct roots and missing process ownership remain rejected.

Codex CLI question coverage uses the production proxy, a subscription-aware mock app-server, an independent control connection, and the IM engine. `cli_proxy_questions_can_be_answered_from_attached_or_background_im` verifies direct attached questions, background Attach reminders, read-only attachment with an existing CLI writer, and answer delivery. Registry/core regressions cover duplicate requests, reply-route identity, CLI resolution, numeric/string RPC IDs, and no stale question after attachment. Real CLI queue shortcuts and live IM clients still need manual acceptance.

Background recipient restart regressions reopen a disk-backed SQLite database and verify delivery without new IM input, both without attachment and after detach. Core/storage tests cover owner updates, bot identity changes, disabled notifications, and absent channels. The Codex mock app-server test `restarted_engine_receives_discovered_background_completion_without_new_im_input` exercises periodic discovery through completion delivery and asserts that observation never calls `thread/resume`. Real Codex proxy reconnection and live IM delivery after a service restart remain manual acceptance checks.

Async CLI question coverage also exercises `agentMessage.questions` on `item/completed`, including proxy observation, attached/background IM rendering, and sending selected answers as ordinary follow-up input. Ordinary message output remains intact. These questions are not JSON-RPC requests; they use a separate internal interaction identity. Answering one is an explicit user action allowed through a read-only attachment, without enabling general session mutation. The CLI question editor is local UI state: IM answers do not clear its local question list. Previously emitted questions from before the proxy began observing are not reconstructed from history.

### Empty Codex attachment recovery

`empty_attach_recovers_*` mock app-server tests attach before the first rollout exists, then materialize the first native turn before subscription recovery. They cover both successful resume and active-writer rejection, completed and in-progress turns, input/output rendering, and continued live or observed completion. The mock retains per-connection subscriptions. Real Codex and IM delivery remain manual verification boundaries.

## Session responsiveness acceptance

The September 15, 2026 audit covers the user-visible path from an IM operation to
feedback and continued control. Component timing is not a substitute for this
path. The [lifecycle guide](session-lifecycle.md) describes the implementation;
[performance results](performance.md) retain workload sizes and exclusions.

| User-visible requirement | Current automated evidence | Evidence boundary |
| --- | --- | --- |
| Attach an empty Codex session, then see its first input and output before completion | `empty_attach_recovers_*` in the mock app-server integration; nonzero update-interval regressions | Exercises subscription recovery and live/observed events, not a real model or IM display |
| Preserve reasoning, process output and Stop when reposting the latest message | `last_reposts_live_card_with_process_content_and_moves_stop_and_updates`, `last_restores_completed_card_from_cold_output_without_losing_reasoning` | Checks rendered views and action ownership |
| Socket closure reports exit without entering a native-new wait | `registry_disconnect_reports_exit_while_process_is_still_alive`, `registry_exit_is_distinct_from_delayed_native_new` | Checks production proxy/adapter events with isolated sockets |
| Exit and native `/new` do not wait for background history | `registry_exit_does_not_wait_for_background_history`, `registry_new_does_not_wait_for_background_history` | Deliberately withholds history replies |
| Input gets feedback while its backend acknowledgement is pending | `feishu_message_traverses_channel_engine_and_codex_then_updates_feishu` | Production runtime and Feishu adapter reach a local HTTP endpoint within the fixture's 500 ms deadline; this is not screen visibility |
| Detach remains available during a pending initial turn | `feishu_detach_does_not_wait_for_pending_turn_start` | Production transport/runtime with held Codex request |
| Menus and detached-session unsubscribe do not occupy the conversation indefinitely | `slow_menu_*`, `detach_feedback_and_next_attachment_do_not_wait_for_old_unsubscribe` in core engine tests | Holds optional requests; checks the 50 ms fast-path behavior with scheduling tolerance |
| Initial attachment stays operable during subscription or history stalls | `runtime_initial_attachment_*`, `attachment_history_*` | Holds initial requests; covers cancellation, exit, `/new`, cross-conversation ownership, ordered input across reload, and content arriving before the binding |
| Reattachment preserves cancellation, input order and reload continuity | `runtime_reattach_*`, `runtime_stop_cancels_input_waiting_for_reattachment` | Holds cleanup; verifies stale completion rejection, unsent failure receipts and new input after Stop |
| Other conversations keep progressing under load | `engine_runtime_fixed_load_preserves_isolation_and_admission_bounds` and flood/outbox regressions | Bounded local fixtures; does not guarantee throughput above the configured capacity |
| Pi/OMP retain native event order and Stop during pending commands | Bridge runtime/host tests and native-new IPC coverage above | Optional installed-host tests are separate from default CI |
| Claude lifecycle and completion recover without repeated full transcript parsing | Claude hook/mailbox tests, transcript projection regressions and benchmark | Hook-based completion does not provide individual assistant-token streaming |

The core engine test names refer to `crates/agentix-core/tests/engine.rs`; proxy
integration names refer to `crates/agentix-codex/tests/mock_app_server_integration.rs`.
The Feishu tests are in `crates/agentix/tests/channel_codex_e2e.rs`. These tests
establish the listed dependencies and ordering, not a universal response deadline.

### Remaining acceptance work

- Real IM visibility remains unmeasured. Use an explicitly designated test
  conversation and isolated agent sessions; record the exact binary/host/channel
  versions and repeat empty attach, active/completed exit, resume, `/new`, queued
  input and Stop. Capture input time, first visible acknowledgement, first visible
  content, completed card and next accepted action, plus message/session IDs.
  Record both median and tail samples, normal traffic and representative background
  traffic. Service/API timestamps alone cannot establish when the client displayed
  a card. Check duplicate/lost input, reasoning, Stop ownership and final state
  alongside timing; provider computation time must be reported separately.

A passing CI matrix covers the automated rows only. Neither a 50/100 ms internal
fast path nor a green mock suite proves that a real user perceives no delay.

Background completion cleanup regressions cover channel removal across runtime inheritance, draining archival, cancellation before optional I/O, and revision reservation before cleanup. Attach regressions verify token reuse, rejected final edits, cancellation, consumed-token preservation, binding changes, and disabled `Attached` state. Core tests assert loading/missing notices in structured sections; `background_content_notices_survive_actual_wire_serialization` checks the complementary Feishu send/update HTTP payloads. Live-client acceptance remains manual.

Codex background integration assertions wait for completion workers before checking root notifications, subagent suppression, source-lookup failure, detached sessions, restored recipients and native goal input. A source-query failure is isolated from the already successful local completion handler.

Combined slow-history/slow-delivery regressions verify history retry while a loading send waits, independent fast-recipient finalization, four-recipient fan-out bounds, cancellation of history and active sends, no queued starts after cancellation, omission of obsolete loading cards, and no duplicate final send after an uncertain loading send.

## Taskix Jev routing

The 2026-09-21 audit covers routing advice through guarded Taskix mutations.
Taskix owns lifecycle invariants and bounded snapshots; the plugin owns host
adaptation, Jev calls and optional observation writes. Semantic advice cannot
replace Taskix revision checks, dependency gates or review policy.

| Requirement | Automated evidence | Boundary |
| --- | --- | --- |
| Disabled or incomplete Jev configuration preserves the legacy path | `jev-runtime.test.mjs`, `jev.test.mjs` | No Jev HTTP request; credentials are synthetic |
| Model/configuration, confidence, ambiguity, timeout and payload budgets | `jev.test.mjs`, `jev-runtime.test.mjs` | Mock provider responses; no accuracy guarantee |
| Bounded candidates and critical truncation | `crates/agentix-task/tests/routing_snapshot.rs` | Real SQLite; terminal Tasks excluded before limit, history excerpts marked separately |
| Confident route includes checked revision | `jev-runtime.test.mjs` and real CLI routing fixtures in `integration.mjs` | Hook output is advice; Agent must execute the supplied guard |
| Main-Agent fallback and stale-write rejection | `jev-runtime.test.mjs`, eight main-Agent fallback cases in `integration.mjs` | All four actual host entrypoints, local HTTP 503, real CLI and SQLite; semantic selection is deterministic in CI |
| Fallback avoids classifier delegation and snapshot files | `jev-runtime.test.mjs`, `package.test.mjs` | Codex/Claude/Pi/OMP adapters; bounded summaries retain assignment and candidate references |
| Compact fallback, escaped-text budgets, large candidate sets and sensitive-field exclusion | `routing-context.test.mjs`, `jev-runtime.test.mjs` | Pure rendering; 100% line, branch and function coverage for `routing-context.mjs` in the 2026-09-22 audit |
| Standalone discussion helper startup and packaged execution | `discussion.test.mjs`, packaged discussion CLI case in `integration.mjs` | Independent Node process, installed-path aliases, real guarded attachment; no runtime import cycle |
| Followup preserves old dependencies and review policy | `integration.mjs`, `crates/agentix-task/tests/task_system.rs` | Real Taskix lifecycle writes |
| Metrics default-off, privacy, worker timeout, lock contention and concurrent writes | `jev-metrics.test.mjs`, `tests/fixtures/metrics-process.mjs` | Best-effort writes may be lost; no task database writes |
| Node writer to Rust report/list/label interoperability | `plugin_entrypoints_execute_the_compiled_taskix` in `crates/taskix/tests/cli.rs` | Default Rust CI sets `TASKIX_TEST_METRICS_BIN`; standalone Node runs skip interop without it |
| Full hook latency includes optional persistence | `real CLI prompt latency` in `integration.mjs` | Mock HTTP; diagnostic samples, not percentile guarantees |

Run the scoped suites from the repository root:

```sh
cargo test -p agentix-task -p taskix --all-features
cargo clippy -p agentix-task -p taskix --all-targets --all-features -- -D warnings
TASKIX_TEST_METRICS_BIN="$PWD/target/debug/taskix" node --test plugins/taskix-manager/tests/*.test.mjs
```

For the isolated fallback renderer's coverage report:

```sh
node --test --experimental-test-coverage \
  --test-coverage-include='**/routing-context.mjs' \
  plugins/taskix-manager/tests/routing-context.test.mjs
```

This coverage percentage applies only to the renderer, not the entire runtime or
semantic model decisions. Host regressions separately cover disabled routing,
uncertain/low-confidence responses, service failure, incomplete evidence, deadlines,
assignment preservation, cancellation, receipt failure and stale revisions. Optional
native-client and Homebrew acceptance remain separate from default automated tests.

Jev acceptance uses deterministic mock responses, including confidence, ambiguity,
malformed responses and transport failures. No real Jev endpoint or API key is
required. Real provider availability, model accuracy and confidence calibration
are outside this delivery's test scope, rather than missing acceptance checks.
Integration fixtures retain real Taskix CLI subprocesses, SQLite and guarded
lifecycle transitions to verify how the application handles those responses.
Installed Codex/Claude/Pi/OMP hook delivery and repeated production latency samples
remain separate live-host checks. Mock tests do not measure model accuracy.

## Discussion attachment and scaling

| Boundary | Executable coverage |
| --- | --- |
| Codex, Claude, Pi and OMP discussion → new Job, PENDING_REVIEW follow-up, or ACTIVE Job continuation (12 combinations) | [Real CLI host integration](../plugins/taskix-manager/tests/integration.mjs): full original messages, unrelated-turn exclusion, original Prompt preservation, late output deduplication, and generated Obsidian notes |
| Jev selection and explicit agent fallback | [Discussion helper tests](../plugins/taskix-manager/tests/discussion.test.mjs): deterministic model responses, disabled/unavailable/uncertain/incomplete context, timeout, stale snapshots and targets; real CLI continuation paths use agent-selected explicit attachment |
| Transactional selection, concurrent writers, malformed guards, restart/expiry, stable order and ownership | [Discussion storage tests](../crates/agentix-task/tests/discussion.rs) |
| Draft capture with an inaccessible output lock, no deserialization of unrelated Job bodies, incremental bound capture, unchanged replay, and deletion cleanup | [Discussion storage tests](../crates/agentix-task/tests/discussion.rs), using isolated SQLite databases and failure injection |
| Interrupted Claude transcript capture | [Discussion helper tests](../plugins/taskix-manager/tests/discussion.test.mjs) through the production hook runtime |

The host matrix uses actual plugin entrypoints and taskix subprocesses with temporary vaults. Model decisions and native host event delivery are controlled fixtures; this does not establish live model accuracy, installed-client event delivery, or production deployment.

Run the opt-in storage scaling check with:

```sh
cargo test -p agentix-task --test discussion discussion_batch_scaling_benchmark -- --ignored --nocapture
```

On one macOS ARM64 debug run with Rust 1.95, attaching 1,000 messages took approximately 9.97 seconds before batched merging and 19 milliseconds afterward; 4,000 messages took 77 milliseconds afterward. These are illustrative local measurements, not timing assertions or service latency guarantees. The benchmark verifies message counts and measures staging, attachment, and unchanged replay separately. It excludes Obsidian projection and model/network calls. Routine tests assert scope and ordering contracts instead of fragile wall-clock thresholds.

### Native terminal command discovery

`agentix-multiplexer` runs isolated child-process regressions with conflicting service and login PATH commands. They verify login PATH precedence, explicit executable PATH inheritance, shell startup noise, paths with spaces, literal argv and server prefixes, removal of inherited tmux selectors, non-replay of executed failures, rejection of invalid/failed shell output, and bounded shell lookup. `agentix-tmux` verifies that inventory and process queries both use the login PATH. These tests require no live IM channel or user terminal. Real Codex terminal tests remain opt-in.

### Jev references across turns

The conversation and Jev tests cover bounded multi-turn transcript reads on
Codex/Claude, eight-message Pi/OMP history, head/tail UTF-8 excerpts, serialized
history and total-request budgets, and preservation of candidate evidence.
Fixtures for "这样修改" and "有性能问题吗" verify that earlier referents reach the
request and that a `discussion:JOB_ID` response retains Job context in all four
host adapters without lifecycle writes. Assignment conflicts and stale revisions
still defer. Responses are deterministic mocks; these tests do not measure live
Jev semantic correctness or acceptance rates.

### Opt-in live Jev replay

`tests/jev-live-replay.test.mjs` replays explicitly supplied local Job conversation
fixtures through the current `routePrompt` implementation and configured real Jev
provider. It is skipped unless `TASKIX_JEV_REPLAY_INPUT` is set. Each fixture supplies
`prompt`, prior `history`, a complete routing `context`, and source Job metadata;
its read-only revision runner uses that fixture's snapshot. It does not mutate Jobs.
Set `TASKIX_JEV_REPLAY_OUTPUT` to a private local report path. Results are persisted
after each call and include acceptance, scores, reasons, bytes and elapsed time.

```sh
fish -lic 'env TASKIX_JEV_REPLAY_INPUT=/absolute/private/input.json TASKIX_JEV_REPLAY_OUTPUT=/absolute/private/results.json node --test plugins/taskix-manager/tests/jev-live-replay.test.mjs'
```

Use prior conversation only and document candidate-state reconstruction, selection
rules, missing Tasks/Inbox, and sampling limits. Acceptance and agreement with a
source Job are not substitutes for human-reviewed correctness. Private inputs and
outputs must remain outside committed source. This replay gathers telemetry directly
without inserting observations into the ordinary routing metrics database.

Suggestion-reference tests preserve candidate source indices through dialogue
truncation and deduplication, include all sources for shared advice, and retain
Agent fallback for ambiguous choices. Live replay reports must retain individual
runs: confidence and acceptance can vary between identical provider requests.

### Jev lifecycle and review policy

| Contract | Coverage | Boundary |
| --- | --- | --- |
| Work review policy, completion destination, upgrade/preservation, approval/rejection/cancellation | `jev-lifecycle.test.mjs` | Mock semantic answers; runtime applies state/revision guards |
| Outcome, waiting recovery, Task selection, retry/reopen/cancel/release | `jev-lifecycle-assessment.test.mjs` | Same bounded context and provider validation; ready requires executor verification |
| Shared helper, disabled zero-I/O path, host routing and structured calls | `lifecycle-runtime.test.mjs` | No lifecycle write by classifier |
| Codex/Claude helper processes and Pi/OMP tool, HTTP, CLI, policy completion matrix, all mutating actions and stale writes | Lifecycle cases in `integration.mjs` | Isolated real SQLite/notes; mock HTTP provider, no semantic accuracy claim |

The actual service's semantic quality requires opt-in historical replay. Deterministic integration coverage does not prove live model accuracy or that a completed task met its acceptance criteria.

Historical replay methods, observed outcomes and local performance measurements are recorded in [Jev lifecycle decisions](jev-lifecycle-decisions.md).

Completion checkpoint integration cases verify ACTIVE -> COMPLETED and ACTIVE -> PENDING_REVIEW after the final Task, including replacement of an earlier required default, disabled/uncertain fallback, read-only classification and stale policy writes. Already PENDING_REVIEW Jobs reject completion assessment.
