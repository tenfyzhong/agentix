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
| IM dashboard → project board → Task ↔ Job navigation; session association and released work; owner/conversation/attachment scoping; ordered menus; project/Job/task pagination and archives; long fenced Markdown, reasons and titles; unavailable-document fallbacks | [IM browsing Engine tests](../crates/agentix-core/tests/support/task_board.rs) |
| Authored Task bodies and Job Goal/Notes in Obsidian, unchanged database/Plan hashes, and symlink escape rejection | [task_system.rs](../crates/agentix-task/tests/task_system.rs), `im_markdown_*` tests |
| Telegram/Feishu dashboard and detail callbacks through actual transports → Engine → authored documents → MarkdownV2/card payloads, project/dashboard return links, attached `/board` and `/jobs`, unchanged task state | [task_browse.rs](../crates/agentix/tests/support/task_browse.rs), run by [channel_codex_e2e.rs](../crates/agentix/tests/channel_codex_e2e.rs) |
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
| Hook mailbox process isolation, failed-event retention, filesystem wakeup and shutdown | [mailbox tests](../plugins/agentix-bridge/tests/claude-mailbox.test.mjs) |
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
