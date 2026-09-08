# Pi, OMP, and Claude bridge review

Reviewed against the current implementation after adding Claude Code. This review covers service assembly and routing, shared wire framing and connection ownership, host lifecycle and delivery state, resource costs, installation, configuration, capabilities, and recovery documentation.

## Architecture decision

Keep one Agentix control listener and one `BridgeHub` connection registry. Route all native backends through `BridgeAdapter`, generated wire DTOs, and the core `SessionOperations` capability/access boundary. Host identity stays qualified in Agentix and native in extensions. Host adapters own execution and delivery receipts; Agentix owns IM authorization, bindings, and presentation.

| Responsibility | Pi / OMP | Claude |
| --- | --- | --- |
| Native connection | In-process extension uses shared `BridgeTransport` | Plugin MCP child uses the same `BridgeTransport` |
| Input | Public host API | `RmuxDelivery` by default, `ChannelDelivery` explicitly selected |
| Output | Public streaming/tool events; host-specific settled event | Process-scoped hooks and completion; optional Channel reply tool |
| Delivery state | `DurableQueue` and native custom-entry journal | `ClaudeSession` and private checkpoint/incremental journal |
| History | Branch index and native message association | Bounded initial transcript import and subsequently observed turns |
| Remote controls | Advertised host API capabilities | Prompt, history, status, uncertain receipt recovery |
| Terminal workspace | Shared Rust rmux integration | Same workspace integration; rmux input additionally verifies the original Claude pane |

Share transport and protocol machinery, but keep the two session models separate. Pi/OMP have native queue/steer/abort APIs and branch-aware logs. Claude relies on hooks and an explicit delivery receipt and does not expose those controls. Forcing both into one queue implementation would obscure these differences. The legacy Pi RPC subprocess library remains outside `serve`; new integrations belong on the live bridge path.

## Corrected findings

| Finding | Correction | Evidence |
| --- | --- | --- |
| Default rmux MCP server exposed Channel-only capability, tools, and instructions | Expose them only in Channel mode; reject direct calls in rmux mode | Both MCP modes exercised by `claude-server.test.mjs` |
| Claude rewrote every prior turn and receipt on each save | Append changed turns/receipts; read legacy checkpoints; repair incomplete journal tails | `claude-store.test.mjs`, [state benchmark](performance.md#claude-and-shared-transport-review) |
| Failed persistence could retain an unsent request, allow an unsafe clear, lose completion, or throw from a timeout callback | Roll back unsent ownership/failed completion/clear; preserve uncertain delivery and always settle the caller | `claude.test.mjs` |
| Hook batch was deleted before processing, with ten idle polls per second | Watch filesystem changes with fallback; delete each event after successful handling | `claude-mailbox.test.mjs`, MCP child integration |
| Rust registration held the global registry mutex while writing acknowledgement | Reserve only the session identity with cancellation-safe cleanup; perform write outside the lock | `bridge_hub.rs` stalled registration and duplicate/cancellation test |
| Socket chunk limit was mistaken for per-frame limit; fragmented frames copied prefixes repeatedly | Shared frame decoder validates each frame and assembles it once | `framing.test.mjs`, transport/runtime integration |
| Transcript import could accept an unterminated final record | Require a terminating newline | `claude-history.test.mjs` |
| Config example, diagrams, command lists, installation notes and coverage map omitted Claude or described Channel-only input | Include all four backends and describe each host's actual capabilities, storage, startup and recovery | Updated configuration and documentation links below |

The existing offline-startup regression also verifies that unavailable saved bindings do not prevent the Unix listener from starting. Reconnection restores observed state without replaying prompts. Existing Pi/OMP tests cover FIFO order, uncertain reload, append failure, session switch during asynchronous operations, branch identity, completion after reconnect, and host-specific settled events.

## Verification and operational limits

See [integration coverage](integration-coverage.md) for executable tests and the final verification run, and [performance](performance.md) for measured workloads and exclusions. Native smoke tests use isolated host configuration and no external model service. The real rmux test verifies draft clearing and submission through a controlled terminal fixture; it does not claim universal compatibility with future Claude UI layouts. Real account/provider permissions and live IM delivery remain environment acceptance checks.

Receipts are retained for duplicate suppression, so storage and startup replay grow with session lifetime. Claude's private store records observed hooks; it is not an ongoing mirror of native transcript changes made without the plugin/hooks. An uncertain receipt is never automatically replayed. rmux pane checks and input-clearing delays are intentional delivery safeguards.

Use the [configuration example](../config/agentix.example.toml), [operational guide](development-and-operations.md), [commands](usage.md), [native protocol](host-protocol.md), [architecture](architecture.md), and [Claude delivery guide](claude-code.md) as the aligned entrypoints.
