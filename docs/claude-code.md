# Claude Code plugin bridge

Claude Code connects its original terminal session to Agentix through the `agentix-bridge@agentix` plugin. The plugin starts a local MCP subprocess, which connects outbound to the existing `~/.local/share/agentix/control.sock`. Agentix still owns IM credentials, authorization, session bindings, and card rendering.

## Delivery modes and compatibility

`AGENTIX_CLAUDE_DELIVERY` accepts exactly three modes. It defaults to `auto`; `rmux` and `tmux` are not delivery-mode values.

| Mode | Selection | Behavior |
| --- | --- | --- |
| Auto | Unset or `auto` | Read-only probes verify the inherited socket, pane and original process ancestry. Exactly one of rmux/tmux must match before any input is sent. |
| Multiplexer | `multiplexer` | Uses the effective service `multiplexer.kind` received during Bridge registration, refreshed on every reconnect. Missing or invalid configuration rejects registration. |
| Channel | `channel` | Uses Claude Channels with the startup flags below. No terminal multiplexer is needed for prompt delivery. |

Terminal modes require installing the relevant rmux/tmux binary on PATH and starting Claude inside it. All modes require the bridge plugin. There is no fallback to another adapter after submission or an uncertain delivery.

The service configuration is global, shared by every agent:

```toml
[multiplexer]
kind = "tmux" # default: rmux
working_dir = "~"
```

Both settings require restarting Agentix when changed. Per-agent working-directory keys have been removed.

## Install from a local checkout

Use Node.js 22 or later and a Claude Code release supporting exec-form command hooks (tested with 2.1.236). For the default non-Channel mode, install rmux or tmux and make it available on PATH. The MCP SDK is bundled; plugin users do not need `npm install`. The default prompt delivery detects the original terminal and does not require Channels.

From the Agentix checkout:

```sh
claude plugin marketplace add .
claude plugin install agentix-bridge@agentix
```

The marketplace is named `agentix`, matching Taskix Manager. You can install either plugin independently or both together. No official marketplace submission is required. If you already registered `agentix` from another checkout, update that marketplace source to the checkout containing this plugin before installing.

Add the backend to the Agentix configuration:

```toml
[agent.claude]
command = "claude"
session_dir = "~/.claude/projects"
```

`session_dir` is the allowed transcript root, not a directory passed to Claude as a session-storage flag. Set it to your actual Claude projects directory when using a custom Claude configuration directory. It can coexist with `[agent.codex]`, `[agent.pi]`, and `[agent.omp]`. There is no backend `kind` field.

## Start Claude Code

### Manual startup with rmux or tmux

Start or restart Agentix. For an unreleased local build, run `cargo build -p agentix` and `./target/debug/agentix serve` from the worktree; a previously installed Homebrew binary may not support the new backend or named configuration tables.

In an rmux or tmux terminal, change to your project directory and start Claude normally:

```sh
claude
```

Create or attach a workspace with `tmux new-session -A -s claude-work` (or the corresponding `rmux` command), then run `claude` there. The bridge uses inherited `TMUX` and `TMUX_PANE` to address the original socket and pane, including a custom socket. A regular terminal outside a multiplexer can report history and events but cannot receive prompts through terminal delivery.

No `--channels` or `--dangerously-load-development-channels` argument is needed. If you saved the earlier fish wrapper that always adds the flag, remove it with `functions --erase claude` and remove `~/.config/fish/functions/claude.fish` only if it is that wrapper. For a single launch, `command claude` bypasses the wrapper.

### Start from IM

Send `/rmux claude` or `/tmux claude` in IM, matching `multiplexer.kind`, and create a terminal. Agentix launches Claude without Channel flags; `agentix-bridge@agentix` must already be installed for that user. Complete any ordinary trust or permission dialogs in the terminal.

### Attach the session

Keep the original terminal running. Use `/sessions claude` in IM to select and attach the session. Leave Claude idle in insert mode, then send an ordinary message in IM. Before pasting, the delivery adapter clears any existing terminal draft with Ctrl+C and verifies that the input box is empty. This discards the terminal draft, including multiline text, so it is not submitted together with the IM message. An already empty input box is left alone. The adapter then pastes and submits the IM text through the original terminal. A matching `UserPromptSubmit` hook confirms receipt; the `Stop` hook reports the answer and completes the turn. Model cooperation with an acknowledgement tool is not required for terminal delivery.

## Channels unavailable with third-party API configurations

A Claude Code session using a third-party API such as DeepSeek can show:

```text
--dangerously-load-development-channels ignored (plugin:agentix-bridge@agentix)
Channels are not currently available
```

In the previous Channel-only implementation, hooks still reported terminal activity to IM, but IM prompts never reached Claude: the two directions use different mechanisms. The development flag bypasses the plugin allowlist only; it does not enable an unavailable Channel feature. Official Channels require supported Anthropic authentication and applicable organization settings.

**The default terminal delivery avoids this dependency.** It submits ordinary terminal input without changing Claude's authentication or provider configuration. The plugin still connects to Agentix's configured Unix or loopback TCP endpoint for requests, events, history, and delivery receipts.

### Start with Channel delivery (optional)

Channel delivery remains supported alongside the default auto adapter. To select it explicitly, start Claude with:

```sh
AGENTIX_CLAUDE_DELIVERY=channel claude --dangerously-load-development-channels plugin:agentix-bridge@agentix
```

This mode requires Claude to accept the development-channel confirmation and enable Channels. It uses `agentix_acknowledge` and `agentix_reply`. These tools, the Channel capability, and Channel-specific model instructions are exposed only in Channel mode; terminal mode advertises none of them. There is no automatic fallback between adapters: switching after an uncertain delivery could duplicate a prompt. Omit the environment variable for the default auto adapter.

### Terminal delivery limitations and recovery

- The adapter checks the pane's original process ancestry, active command, cursor, and surrounding prompt borders, then verifies the cleared input row. Existing drafts are cleared before submission. A dialog, copy mode, non-insert Vim mode, unrecognized layout, or unsuccessful clearing prevents submission. An already active turn is rejected.
- Text is loaded into a uniquely named terminal buffer through stdin, pasted with bracketed-paste support, then submitted with Enter. It is not interpreted by a shell. Messages are limited to 64 KiB; terminal control characters and leading slash/bang commands are rejected. Newlines are supported.
- Clearing uses the default Claude Ctrl+C binding only for a detected nonempty draft; it is not the `/clear` command and does not reset conversation history. These checks are best effort, not an atomic terminal lock. Do not type or change pane state while Agentix is submitting. UI changes or another user typing between checks can still interfere. Handle permissions, stop, and steering locally.
- Successful terminal commands alone do not confirm delivery. The bridge waits for a matching prompt hook. A partial paste or acknowledgement timeout is retained as `delivery_uncertain` and is never replayed automatically. Inspect the terminal and `/history`; remove any unsubmitted pasted draft locally before using `/queue clear` and sending a new request. Clearing a receipt does not cancel text already submitted.
- If resuming a session with an uncertain receipt from the old Channel adapter, inspect it and use `/queue clear` before retrying with terminal delivery. `/status` identifies the active delivery adapter and unresolved receipts.

## Protocol and lifecycle

The registration acknowledgement includes `result.multiplexer.kind` from the running service configuration. The plugin does not reread a separate TOML file. The plugin reuses bridge protocol version 2, newline-delimited JSON registration, requests, responses, snapshots, and events on the existing control socket. MCP JSON-RPC is confined to the Claude-to-plugin stdio connection. No additional socket listener is created.

Exec-form hooks and the MCP server run as children of the same Claude process. They exchange hook events through a private mailbox keyed by parent PID and process start time, so two terminals in the same working directory remain separate. Filesystem notifications wake the consumer; a one-second fallback scan covers unavailable or missed notifications. An event is removed only after handling succeeds, so a failed state write leaves it and subsequent events available for retry. SessionStart supplies the real session ID, transcript path, and working directory before or after MCP startup. A session switch replaces the bridge incarnation. Agentix reconnection preserves it. Reconnecting Agentix while the plugin remains running reconciles the turns observed by that plugin. The private store is not a continuous mirror of edits made to the transcript while the plugin and its hooks are absent; inspect native history locally if activity was not observed.

The default state directory is `~/.local/share/agentix/claude`; `AGENTIX_CLAUDE_DATA_DIR` overrides it. `AGENTIX_CONTROL_ENDPOINT` overrides the Agentix control endpoint, as for Pi/OMP: use `unix:///custom/control.sock` or `tcp://127.0.0.1:4150`, matching `[server].endpoint`. State files contain conversation text and delivery receipts and are created with owner-only permissions. Claude transcript files are read, never modified. On first registration the plugin imports up to 16 MiB of the transcript tail, excluding an incomplete final JSONL record. Subsequent observed turns and receipts are saved independently for recovery. Existing `sessions/<hash>.json` checkpoints remain readable; changes are appended to the adjacent `.jsonl` journal instead of rewriting all history and receipts on every hook. Startup replays the journal and truncates an incomplete final append; malformed complete records fail explicitly. These files retain conversation text and deduplication receipts for the session lifetime. Back up both files together. Local filesystem writes are synchronous but do not issue a per-record `fsync`; this is process-restart recovery, not a power-loss durability guarantee.

The supported capabilities are `prompt`, `history`, `status`, and `queue_control`. Stop, steering, remote model changes, approval relay, and queued prompt submission are not advertised. Queue controls only reconcile uncertain deliveries; they do not manage Claude’s native event queue. Replies are delivered through completion hooks (or the reply tool in Channel mode), rather than token-by-token streaming. The terminal remains the place to interrupt Claude or handle permissions.

The delivery interface is `send({ request_id, text, signal })`; `TerminalDelivery` is the default implementation and `ChannelDelivery` is an explicit alternative. Transport writes do not acknowledge delivery. The bridge waits up to seven seconds for the matching prompt hook (terminal delivery) or explicit tool acknowledgement (Channel) before returning `delivery_uncertain`. It retains the receipt and does not resend automatically, including after a restart. Late acknowledgements reconcile the original request. An unresolved delivery prevents another remote prompt; inspect the session and `/status`, then use `/queue clear` to explicitly abandon the uncertain receipt before submitting a new request. This does not cancel input already submitted to Claude. A reply tool call updates the answer but does not complete the turn, because Claude may continue using tools afterward.

## Development and verification

```sh
npm ci --ignore-scripts
node plugins/agentix-bridge/claude/build.mjs --check
node --test plugins/agentix-bridge/tests/claude*.test.mjs
AGENTIX_TEST_NATIVE_HOSTS=1 node --test plugins/agentix-bridge/tests/claude-native.test.mjs
cargo test -p agentix-bridge
```

The native smoke test installs the plugin into an isolated Claude configuration and verifies registration from the original host PID. It uses an unreachable local model endpoint and does not call an external model service. Protocol tests exercise acknowledgements, replies, completion, and the real Rust bridge adapter without model inference. To update the bundled MCP SDK after changing its locked development dependency, run `node plugins/agentix-bridge/claude/build.mjs` and commit the generated bundle.

References: [Channels](https://code.claude.com/docs/en/channels), [Channel protocol](https://code.claude.com/docs/en/channels-reference), and [hooks](https://code.claude.com/docs/en/hooks).
