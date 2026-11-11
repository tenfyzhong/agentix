# Antigravity CLI

Agentix supports the native `agy` CLI as backend `antigravity`. The acceptance
baseline is agy 1.5.5 on macOS. Native process identity currently requires Unix
`ps`; Windows native host support is not implemented. It attaches to the original CLI process through
native hooks and an MCP sidecar; it does not start a second conversation to
answer an attached session's prompts.

## Install

Install `agy`, Node.js, Agentix and Taskix, then complete agy's normal sign-in and
initial setup in your own terminal. Start `agy` in tmux, rmux or Rex for remote
input and controls. Existing login and theme settings remain in your home.

From an Agentix checkout whose current branch exists on origin:

```sh
make plugin HOSTS=antigravity
```

This installs both `agentix-bridge` and `taskix-manager`. The installer fetches
the current remote branch, builds native-only packages and runs
`agy plugin validate --strict` before replacing either plugin. It preserves each
plugin's enabled state and leaves unrelated plugins alone. Reported partial
failures restore the previous packages; this is not crash-atomic or safe for
concurrent installers. Restart agy after installation or update.

```sh
agy plugin list --json
make remove-plugin HOSTS=antigravity
```

Removal is idempotent and only removes these two named plugins. For development
against unpushed local changes, use
`node scripts/antigravity-plugin.mjs install --local /absolute/checkout`.
The raw multi-host plugin directories are not native install targets: the
packager filters foreign host manifests and preserves the full Taskix workflow
as a referenced skill document.

## Configure and attach

```toml
[agent.antigravity]
command = "agy"
session_dir = "~/.gemini/antigravity-cli/brain"
state_dir = "~/.local/share/agentix/antigravity"
```

Restart Agentix and run agy with the plugins enabled. Use `/sessions` in
Telegram, Feishu or Slack to attach even before the first message. Empty processes
appear as `New session (PID ...)`, with distinct temporary IDs derived from the
process ID and start time. The session picker also shows the terminal session,
window and pane when the configured multiplexer can resolve that process.
Processes in the same directory remain separate; input delivery verifies the
original process and pane before sending. When the first input creates agy's
native conversation ID, the existing attachment and pending acknowledgement move
to it without sending the message again. Restart agy after upgrading its plugin.
For a custom native `session_dir`, also export the matching absolute
`AGENTIX_ANTIGRAVITY_SESSION_DIR` in the terminal. Temporary registrations reference
a virtual `.agentix-pending` path under that root; no native trajectory is created.
For custom bridge storage, export `AGENTIX_ANTIGRAVITY_DATA_DIR` in the original
terminal as well. `AGENTIX_CONTROL_ENDPOINT` selects a nondefault Agentix control
endpoint. IM-created sessions receive these settings from the launcher.
Taskix's sidecar defaults to `~/.local/share/taskix/antigravity`; its override is
`TASKIX_ANTIGRAVITY_DATA_DIR`. These stores contain local identity and delivery
receipts. Native trajectory files are read without modification.

## Controls and boundaries

- Prompts, visible replies, history, status and interruption use the original
  session. Delivery verifies the foreground CLI and empty composer before pasting.
  A native invocation acknowledges the submitted user step. Ambiguous delivery
  is retained as uncertain and is never automatically resent.
- `/model` and `/reasoning` use the CLI's current `/model` and `/effort` menus.
  Only choices actually offered by that host are available.
- `/new` opens agy's native new conversation. Its identity is allocated after
  the next message, so the old binding remains usable until the native switch
  is acknowledged. This command does not allocate another temporary ID; the
  startup-only temporary ID is replaced when the first native conversation appears.
- Terminal drafts are preserved. Conditional clearing supports a matching
  single-line draft; clear multiline drafts in the original terminal. Busy,
  changed or unrecognized screen layouts are rejected before input is sent.
  Nondefault composer modes may require returning to the normal empty composer.
- Command and file approvals, file creation/edit cards, and single/multiple
  choice questions with write-in answers are exposed as ordinary IM
  interactions. Each response revalidates the current native card. Unknown
  layouts remain terminal-only; no tool permission is automatically granted.
- There is no automatic prompt FIFO. Queue commands expose uncertain delivery
  recovery; clearing uncertainty does not cancel or resend a native message.
- History projects explicit user text and visible planner responses. Tool
  output and private thinking are not presented as assistant prose. The reader
  bounds work to a 16 MiB tail, 200 turns and 100,000 characters per text field;
  older/truncated history is intentionally unavailable through this projection.

The Taskix MCP tool binds commands to the original native session and executor.
Native hooks route each explicit user step once, capture discussion, renew
leases, and preserve shared plan/review/idempotency rules. Normal Stop records
completion without releasing the host; user cancellation releases ownership.
MCP EOF supplies session-end cleanup because agy has no SessionEnd hook. A
force-killed process still relies on lease expiry.

## Metrics

Operation metrics use component `antigravity`; token counters use source
`antigravity_bridge`. Live native planner steps provide `input_tokens`,
`output_tokens` and `cache_read_tokens` when present. Missing counters stay
unknown; cache reads are reported separately without assuming they are included
in input. Repeated step updates are deduplicated, and history queries and
reconnect snapshots do not replay usage. This is partial observed usage, not
billing or complete session totals.

## Verification

See [integration coverage](integration-coverage.md#antigravity) for automated,
native and performance checks and the remaining acceptance boundaries.
