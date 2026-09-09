# Agentix bridge for Pi, OMP, and Claude Code

This extension connects Agentix to the session running inside your original Pi or OMP process. Attaching, detaching, and restarting Agentix do not start a replacement agent or terminate the terminal.

For Claude Code, install `agentix-bridge@agentix`, configure `[agent.claude]`, and start Agentix as described in the [plugin installation and protocol guide](../../docs/claude-code.md). Then run `claude` in your project directory inside an rmux terminal. IM prompts use rmux input; hooks report replies through the same Agentix socket. Channel flags are not required. See [startup options and third-party API limitations](../../docs/claude-code.md#start-claude-code).

## Install and start

Install the repository using the [Pi or OMP package instructions](../taskix-manager/README.md#prerequisites-and-activation), then restart or reload the host. The repository installs both the task extension and the bridge. For a local checkout you can explicitly load one entrypoint:

```sh
pi -e /path/to/agentix/plugins/agentix-bridge/extensions/pi.ts
omp -e /path/to/agentix/plugins/agentix-bridge/extensions/omp.ts
```

Enable each backend with its own named table. The table name determines the backend; no `kind` field is needed:

```toml
[agent.codex]

[agent.pi]
session_dir = "~/.pi/agent/sessions"
rmux_directory = "~/work"
# Optional explicit entrypoint for terminals created using /rmux:
# bridge_extension = "/path/to/agentix/plugins/agentix-bridge/extensions/pi.ts"

[agent.omp]
session_dir = "~/.omp/agent/sessions"
rmux_directory = "~/work"
```

Only live, registered connections whose session file is under `session_dir` appear in Agentix. Start `agentix serve` to open the shared listener; extensions may start before or after the service. History queries return at most 20 turns per page; Pi/OMP visibly shorten exceptionally large message text in history responses. Session IDs are qualified as `codex:<id>`, `pi:<id>`, `omp:<id>`, and `claude:<id>`. `/sessions pi`, `/sessions omp`, and `/sessions claude` filter the picker. A bare ID works only when exactly one backend owns it. Taskix continues to store the native host ID.

Existing unqualified bindings must first be migrated by starting Agentix once with the original single-backend configuration. Multiple backends never guess the owner of legacy bindings.

## Controls

Pi and OMP expose prompts, streaming replies, history, stop, rename, model selection, reasoning level, compaction, status, skills, and Git diff. The service also provides IM detach/exit and rmux creation. Commands are shown according to the running host's capabilities. Model choices come from that host's available model registry; reasoning changes are applied by the host and report the effective level.

Ordinary messages sent during a turn enter a durable FIFO. `/steer <text>` explicitly steers the active turn. `/stop` interrupts it and pauses pending work. `/queue`, `/queue resume`, and `/queue clear` inspect, resume, or discard pending work. Clearing pending work does not interrupt a running delivery. Queue records belong to their original session and are not replayed into a fork. Native terminal queues are separate; avoid mixing queue mechanisms when ordering matters.

A disconnected client does not trigger replay. If an extension reload discovers an unconfirmed delivery, the queue stays paused. Inspect history before clearing and submitting again. Requests are deduplicated by their IDs; deliberately submitting the same text as a new message is a new request.

`/rmux pi`, `/rmux omp`, `/rmux claude`, and `/rmux codex` select a launch backend for the current chat. With multiple backends and no attachment, `/rmux` offers a picker. The service waits for a new live bridge associated with the created pane before attaching. A timeout leaves the terminal open for inspection.

Third-party extension dialogs and approval prompts stay in the original terminal. Clear, fork, plan, goal, review, Fast mode, and MCP management are not exposed for Pi/OMP. Claude Code uses a hook-backed session adapter with default rmux delivery and an optional Channel adapter; see the [Claude guide](../../docs/claude-code.md) for its supported capabilities. The Taskix Manager plugin can be installed independently.

## Transport and troubleshooting

Extensions reuse the existing Agentix control listener at `~/.local/share/agentix/control.sock`. The same listener serves CLI `session`, `sessions`, `call`, and `claim` requests and upgrades `register` requests to native session connections. It also handles `inspect` for diagnostics. No separate bridge socket or credential file is created. Pi/OMP persist bridge state in native session logs; Claude uses the private state directory described in its guide.

The Unix socket uses mode 0600. Native registration uses the same local access boundary as existing CLI control requests; it does not require a separate token. Register frames carry protocol version 2, backend, native session ID, instance UUID, PID, capabilities, and lightweight session metadata. History and queue state are requested separately. The service validates backend session roots and rejects duplicate live owners.

For a custom endpoint, configure Agentix and the host consistently:

```toml
[server]
endpoint = "unix:///path/to/custom-control.sock"
```

```sh
AGENTIX_CONTROL_ENDPOINT=unix:///path/to/custom-control.sock pi
AGENTIX_CONTROL_ENDPOINT=unix:///path/to/custom-control.sock omp
```

The default needs no environment variable. `AGENTIX_BRIDGE_DIR` is no longer used. Restart Agentix and reload the host extensions when upgrading from the separate bridge listener. Existing old bridge files are ignored.

If Agentix is unavailable, extensions retry in the background without blocking the CLI. Reconnection preserves the original process/session and refreshes Agentix state; it does not resend prompts. Service shutdown closes native connections, and extensions reconnect when the control listener returns.

Native bridging requires a Unix `server.endpoint` (Linux/macOS); the existing TCP control transport remains available for non-native configurations. A missing session usually means the extension is not loaded, its session file is outside `session_dir`, or its `AGENTIX_CONTROL_ENDPOINT` does not match the service. A second live connection for an already registered backend/session is rejected until the first disconnects.

The native loader smoke tests have passed with Pi 0.84.4 and OMP 17.3.7. They exercise public extension APIs without model requests. Run them against installed hosts with:

```sh
AGENTIX_TEST_NATIVE_HOSTS=1 node --test plugins/agentix-bridge/tests/native-host.test.mjs
```

The default Rust and Node suites use local fixtures and do not need provider credentials or real IM accounts. See the [host protocol](../../docs/host-protocol.md), [layered architecture](../../docs/architecture.md), and [performance measurements](../../docs/performance.md) for implementation details.
