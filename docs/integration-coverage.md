# Integration coverage

This map connects documented behavior to executable tests. It describes behavioral coverage, not a measured claim of 100% line or branch coverage. Tests use temporary databases, document trees, and local protocol services. The separately enabled desktop test uses an explicitly selected Obsidian vault.

## Running the checks

Use the pinned Rust 1.95.0 toolchain, Node.js 24+, and npm:

```sh
make check
```

This runs formatting, Clippy with warnings denied, workspace tests, and the plugin's Node tests. `make check` installs locked plugin dependencies. For a focused Cargo run, install them first:

```sh
npm ci --ignore-scripts --prefix plugins/agent-task-manager
cargo test -p agentix-task -p taskcli
node --test plugins/agent-task-manager/tests/*.test.mjs
```

Cargo's `plugin_entrypoints_execute_the_compiled_taskcli` test runs [integration.mjs](../plugins/agent-task-manager/tests/integration.mjs) with the freshly compiled binary. It must not fall back to an installed taskcli or the user's database. Set `TASKCLI_TEST_HOOK_SHELL=fish` to additionally check hook commands through fish; Linux/macOS CI does this. Windows runs commands through `cmd.exe`.

## Tasks, projections, and host lifecycle

| Behavior | Executable coverage |
| --- | --- |
| Claim → Plan → start → done; seven statuses, phase gates, failed transitions, revision checks, stale leases, expiry and resumption | [task_system.rs](../crates/agentix-task/tests/task_system.rs), [taskcli CLI tests](../crates/taskcli/tests/cli.rs) |
| Competing processes, concurrent projection writers, crash after commit, exact idempotent replay, database identity and path validation | [taskcli CLI tests](../crates/taskcli/tests/cli.rs), [task_system.rs](../crates/agentix-task/tests/task_system.rs) |
| Dependency validation, cross-Job prerequisites, one graph node for a shared prerequisite, seven colors, escaped labels, renamed/archived links, removal of legacy Dependencies prose | [job_graph.rs](../crates/agentix-task/tests/support/job_graph.rs), [CLI projection tests](../crates/taskcli/tests/support/projections.rs) |
| Task notes before planning; managed tags/dependencies; local timestamps; freeform bodies and custom properties, CRLF frontmatter, quoted property keys, metadata-only Plan rejection; Board metadata; old meta/Task-list/Plan-path migrations | [tasknotes.rs](../crates/agentix-task/tests/support/tasknotes.rs), [CLI projection tests](../crates/taskcli/tests/support/projections.rs) |
| Dashboard Base/Markdown format changes, read-only formulas, scoped filters, activity ordering, unchanged-file preservation, archive visibility, destination conflicts and partial-publication retries | [dashboard.rs](../crates/agentix-task/tests/support/dashboard.rs), [CLI projection tests](../crates/taskcli/tests/support/projections.rs) |
| Job/Project deletion, active-lease and surviving-dependency rejection, cleanup retry after restart, unowned destination preservation after failed creation/renaming, owned files published before path registration, sequence retention, symlink boundaries | [deletion.rs](../crates/agentix-task/tests/support/deletion.rs), [taskcli CLI tests](../crates/taskcli/tests/cli.rs) |
| Codex/Claude manifest-selected hook commands using real CLI; Stop retains ownership without intake; interruption and shutdown release planning/executing leases; new-token recovery | [integration.mjs](../plugins/agent-task-manager/tests/integration.mjs) |
| Manual Inbox intake after each Job: completion/idle events leave pending entries without Jobs, explicit claim creates one Job, and legacy `hook stop` cannot claim | [Inbox CLI tests](../crates/taskcli/tests/support/inbox.rs), [inbox.test.mjs](../plugins/agent-task-manager/tests/inbox.test.mjs), [integration.mjs](../plugins/agent-task-manager/tests/integration.mjs) |
| Claude ordinary failures, missing flags, and string `"true"` do not release or renew; only boolean `true` requests interruption | [lifecycle.test.mjs](../plugins/agent-task-manager/tests/lifecycle.test.mjs), [integration.mjs](../plugins/agent-task-manager/tests/integration.mjs) |
| Pi/OMP real TypeScript entrypoints, automatic identity/token injection, Plan writes with full IDs and resolved Task prefixes, ambiguous-prefix rejection, errors, aborted processes, retry identity including Job/Project deletion | [runtime.test.mjs](../plugins/agent-task-manager/tests/runtime.test.mjs), [integration.mjs](../plugins/agent-task-manager/tests/integration.mjs) |
| Pi/OMP normal replies and automatic continuations retain executing leases; repeated SessionStart and old-session callbacks do not revoke current ownership | [lifecycle.test.mjs](../plugins/agent-task-manager/tests/lifecycle.test.mjs), [integration.mjs](../plugins/agent-task-manager/tests/integration.mjs) |
| Heartbeat timing, cancellation of in-flight renewal, queued ticks, failed cleanup retry, and new prompts waiting for cleanup | [lifecycle.test.mjs](../plugins/agent-task-manager/tests/lifecycle.test.mjs), [runtime.test.mjs](../plugins/agent-task-manager/tests/runtime.test.mjs), using controlled timers and runners |
| Marketplace resolution, hook discovery without duplicates, entrypoint selection, actual npm pack file list including TaskNotes guide/settings and local documentation links | [package.test.mjs](../plugins/agent-task-manager/tests/package.test.mjs) |
| IM task actions, lease/revision/owner checks, reasons, notifications, delivery retry and durable event cursors | [Engine tests](../crates/agentix-core/tests/engine.rs) |
| IM dashboard → project board → Task ↔ Job navigation; session association and released work; owner/conversation/attachment scoping; ordered menus; project/Job/task pagination and archives; long fenced Markdown, reasons and titles; unavailable-document fallbacks | [IM browsing Engine tests](../crates/agentix-core/tests/support/task_board.rs) |
| Authored Task bodies and Job Goal/Notes in both formats, unchanged database/Plan hashes, and symlink escape rejection | [task_system.rs](../crates/agentix-task/tests/task_system.rs), `im_markdown_*` tests |
| Telegram/Feishu dashboard and detail callbacks through actual transports → Engine → authored documents → MarkdownV2/card payloads, project/dashboard return links, attached `/board` and `/jobs`, unchanged task state | [task_browse.rs](../crates/agentix/tests/support/task_browse.rs), run by [channel_codex_e2e.rs](../crates/agentix/tests/channel_codex_e2e.rs) |
| Configured Telegram startup dashboard menu, exact primary order, alphabetical secondary commands and contextual labels | [application menu tests](../crates/agentix/src/main.rs), [Telegram API/menu tests](../crates/agentix-telegram/tests/telegram.rs), [Engine menu tests](../crates/agentix-core/tests/support/task_board.rs) |
| Telegram/Feishu task actions and reason replies through transport → Engine → SQLite → documents → channel notification | [channel_codex_e2e.rs](../crates/agentix/tests/channel_codex_e2e.rs), with local channel API services |
| Actual Dashboard columns/dates, archive filtering, native link navigation, TaskNotes Kanban columns/cards and task note recognition | [obsidian_smoke.rs](../crates/taskcli/tests/obsidian_smoke.rs), opt-in desktop test |
| TaskNotes installation, settings/status merging, plugin enablement, backups, repeat runs, malformed bundles/configuration, path protection, download errors and rollback | [obsidian_setup.rs](../crates/taskcli/tests/obsidian_setup.rs), [installer tests](../crates/taskcli/src/obsidian.rs); local release fixtures and HTTP server |

The projection CLI tests cross argument parsing, a new process per command, SQLite persistence, and generated files. Library tests provide deterministic clocks and filesystem failure injection. Host integration tests run real taskcli processes behind a minimal event API harness; controlled timer tests cover scheduling races without waiting a real minute.

## Other workspace boundaries

| Behavior | Executable coverage |
| --- | --- |
| Configuration, single selected channel, inactive credentials, Home paths, proxy validation, task-board opt-in | [configuration tests](../crates/agentix/tests/config.rs), [network tests](../crates/agentix/tests/network.rs) |
| CLI control transport, startup/shutdown, logging, argument errors, shell completions | [CLI tests](../crates/agentix/tests/cli.rs), [control tests](../crates/agentix/src/control.rs), [completions](../crates/agentix/tests/completions.rs) |
| Attachment, routing, draining sessions, queues, approval/input flows, retry, restart recovery, notifications and menus | [Engine tests](../crates/agentix-core/tests/engine.rs), [core behavior](../crates/agentix-core/tests/core_behavior.rs), [state/render tests](../crates/agentix-core/tests/state_and_render.rs) |
| FIFO admission, independent inbound/outbound progress, cancellation and backpressure | [message_center.rs](../crates/agentix-core/tests/message_center.rs), channel adapter suites |
| Codex RPC/event sequences, history/queue pagination and fallback, external writers, read-only attachment, process exit/resume, reconnects and background observation | [mock app-server integration](../crates/agentix-codex/tests/mock_app_server_integration.rs), [UDS client tests](../crates/agentix-codex/tests/uds_client.rs), [protocol tests](../crates/agentix-codex/tests/protocol.rs), [observed lifecycle tests](../crates/agentix-codex/src/client/lifecycle_tests.rs) |
| Pi/OMP JSONL RPC subprocesses and session discovery | [Pi adapter tests](../crates/agentix-pi/tests/adapter.rs), [protocol tests](../crates/agentix-pi/tests/protocol.rs) |
| Telegram owner/mention/claim rules, callbacks, API payloads, Markdown, FIFO retries and pacing | [Telegram tests](../crates/agentix-telegram/tests/telegram.rs) |
| Feishu long connection, official protobuf frames, callbacks, credentials, cards, reply lookup, token refresh and rate limits | [Feishu tests](../crates/agentix-feishu/tests/feishu.rs) |
| Full mocked Telegram/Feishu prompt → Engine → Codex → completed channel response | [channel_codex_e2e.rs](../crates/agentix/tests/channel_codex_e2e.rs) |
| rmux navigation, typed SDK requests, local socket exchange and process launch | [Engine tests](../crates/agentix-core/tests/engine.rs), [multiplexer tests](../crates/agentix-codex/src/multiplexer.rs) |
| Native release archives, checksums, version alignment and Homebrew formula transformation | [packaging tests](../crates/agentix/tests/packaging.rs) |

## Desktop and external acceptance

The normal suite needs no live account, model, host installation, or desktop. To run native Obsidian checks, follow [task board validation](task-board.md#validation): open the selected vault in the foreground, enable Bases and TaskNotes with the seven statuses, and supply `TASKCLI_OBSIDIAN_VAULT` and an existing `TASKCLI_OBSIDIAN_PARENT`. The test checks generated link destinations through Obsidian's native navigation API; it does not claim physical mouse or keyboard automation in every theme.

Host installer discovery/trust, real model behavior, host events on actual terminal interruption/exit, live IM credentials/permissions, and a live external rmux daemon require environment acceptance. A mock event test establishes what the adapter does when the event arrives; it cannot establish that every host version emits it. Force-kill and missed-hook recovery retain the lease-expiry fallback. Multi-machine or network-filesystem coordination is outside the supported concurrency model.

CI runs the full workspace suite on Linux/macOS. Windows checks the workspace and runs native TCP control plus task library/CLI/plugin tests. Timestamp tests verify the system-local offset on all platforms. Unix additionally tests process `TZ` overrides; Windows switches the runner system time zone through Tokyo (UTC+09:00), SA Pacific (UTC-05:00), and UTC, with explicit expected offsets and restoration in `finally`. Native Obsidian rendering is opt-in and is not run in CI. When adding a feature, extend the boundary tests and this map; do not describe an unexecuted live check as covered by its mock.
