# Using Agentix

Agentix exposes local coding-agent sessions through Telegram or Feishu. Start `agentix serve`, open the configured bot, and use `/sessions` to find a running session. Select its Attach action, then send ordinary chat messages to prompt the agent.

## Core workflow

1. Start the coding-agent session locally.
2. Run `agentix serve` with the configured backend and IM channel.
3. Send `/sessions` to the bot and attach the session you want to control.
4. Send ordinary messages to prompt the attached agent.
5. Use `/current`, `/history`, and `/queue` to inspect it, or `/stop` and `/detach` to control the connection.

In group chats, mention the bot. Direct messages are accepted only from configured owners, except for a valid first-time `/claim <code>` when the selected owner list is empty. Claim attempts in groups are ignored. A coding-agent session can be current in only one IM conversation at a time.

## Commands

These commands are available according to the conversation's current attachment state:

- `/help` — show the commands currently available
- `/sessions` — list running sessions with their title, status, workspace, and rmux location; use an item's action to attach it
- `/rmux` — browse rmux sessions, windows, and panes, or create a workspace and launch Codex
- `/attach <session-id>` — attach the conversation to a running session
- `/current` — show the attached session and running turn
- `/history`, `/history older`, `/history newer` — browse turns with separate user and agent sections
- `/queue` — inspect the attached Codex session's persistent FIFO follow-up queue
- `/stop` — interrupt the current turn
- `/detach` — remove the current binding without stopping the coding-agent session
- `/cancel` — leave a pending free-text input flow
- any other text — start a turn; while Codex is active, add a follow-up turn to its Agentix queue; other backends steer the active turn

Unknown or malformed slash commands return a parsing error and the same state-aware command list shown by `/help`.

### Codex commands

The following commands are available only while a Codex session is attached:

- `/fast [on|off]` — show or change the current model's Fast service tier when supported
- `/clear [name]` — create and attach a fresh session with the current directory and settings; unavailable during an active turn
- `/exit` — leave the attached session from IM without stopping Codex
- `/diff` — show staged, unstaged, and untracked Git changes
- `/rename [name]` — rename the session; omit the name to provide it in the next message
- `/compact` — start context compaction
- `/fork` — fork the session and attach the fork
- `/model [model-id]` — show or select a model for later turns
- `/reasoning [effort]` — show or select a reasoning effort for later turns
- `/skills` — list enabled skills in the attached workspace
- `/plan [prompt]`, `/plan off` — enter plan mode, optionally with a prompt, or return to default mode; unavailable during an active turn
- `/goal [objective|pause|resume|clear]` — inspect or manage the thread goal
- `/review` — start an inline review of staged, unstaged, and untracked changes
- `/status` — show session, model, execution-policy, token-usage, and goal details
- `/mcp` — list MCP server connection, authentication, and tool status

These extended controls use the Codex adapter; Pi and Oh My Pi report them as unsupported.

## Prompts, queues, and replies

An ordinary message starts a turn when the attached session is idle. While Codex is active, Agentix places new messages in its persistent app-server FIFO queue and immediately reports their positions. `/queue` shows this queue.

Codex CLI's Tab queue is private to the TUI and does not synchronize or deduplicate with the Agentix app-server queue. If both contain input, each queue may submit its next item when a turn finishes, producing back-to-back turns with no shared ordering guarantee. Avoid using both queue mechanisms for the same session at the same time.

Replying to a Telegram or Feishu message adds the earlier message as quoted context before the new prompt. A manually selected Telegram quote takes precedence over the full replied-to text or media caption. Feishu extracts visible content from text, rich-text, and card messages. Slash commands ignore reply context.

## Interactive requests

Agentix renders Codex approval and plan-input requests as separate actionable messages. After an approval decision, Agentix removes or disables the original controls and shows the result in place.

For multi-question input, Agentix presents one question at a time with option buttons and an `Other…` free-text path. It submits all answers together and replaces the controls with an answer summary. If the request is resolved in Codex CLI, Agentix marks the IM request as resolved outside the chat because the app-server notification does not include the selected decision or answers.

Every action token is single-use. Telegram removes a consumed inline keyboard; Feishu leaves the buttons visible but disabled.

## Session and message behavior

Session-specific output uses `title · short-id` in current-session views, history, live-turn headers, command results, queue views, completion notices, and lifecycle notices.

Agentix creates a turn message immediately with `Working 0s`, edits it at most once per second while the turn runs, and preserves its Stop action. Completion, interruption, or failure leaves the final elapsed time in the status line. After an Agentix restart, a restored running turn begins a new locally observed duration because the agent protocol does not expose its original monotonic start time.

When a turn finishes in a session that is not attached to an IM conversation, Agentix notifies authenticated conversations known to the running service and includes a single-use Attach action. Replayed completion events do not create duplicate notices. For Codex, the service discovers running sessions and establishes event subscriptions every two seconds, even when no IM conversation is attached. `/detach` removes the IM binding; a running session is rediscovered for background notifications. Send `/help` to the bot after starting the service if you have no restored binding and want to receive these notifications.

Telegram uses native command menus that change with attachment state. Feishu sends an interactive command card and updates it as the state changes. Contextual commands use a `✌️` marker. `/attach` remains available as typed input, but the normal path is the Attach action returned by `/sessions`.

## rmux workspaces

`/rmux` connects to the local rmux daemon and starts it when needed. You can attach an existing Codex pane or replace an idle shell pane with a new Codex session. Agentix can also create a session, window, or split in `agent.rmux_directory`, which defaults to the current user's home directory.

Before replacing an existing shell pane, Agentix sends `Ctrl-C` to discard any unsubmitted command line. It refuses to replace a pane running a non-shell process. New panes remain visible after Codex exits. Agentix waits for the real Codex session before attaching; if Codex exits early or discovery times out, it reports an error instead of attaching a placeholder.

## Local client

Every `agentix client` command connects to the control endpoint of the running `agentix serve` process. The defaults are `unix://~/.local/share/agentix/control.sock` on macOS and Linux, and `tcp://127.0.0.1:32198` on Windows.

```sh
agentix client sessions
agentix client sessions --limit 10
agentix client call thread/read --params '{"threadId":"019...","includeTurns":false}'
agentix client claim --ttl-minutes 10
```

`client sessions` emits normalized JSON for available sessions. With managed Codex, it includes running standalone and daemon-backed TUI sessions and their rmux locations while excluding stored sessions and orphaned daemon threads. `client call` sends raw Codex JSON RPC through the server's existing app-server connection for protocol diagnostics; JSON goes to stdout and logs go to stderr. `client claim` creates a temporary in-memory owner claim and is not an IM command.

## Restarts and process exits

During graceful shutdown, Agentix checkpoints bindings, removes live controls, restores detached command menus, and sends an offline notice without stopping coding-agent sessions. On startup, it sends an online notice and reattaches sessions that remain available. Stale saved sessions are discarded and reported as detached.

With managed Codex, exiting an attached Codex process temporarily detaches the IM conversation and starts watching for the same session ID. Running `codex resume` for that session restores the app-server subscription and binding automatically. Manually detaching or attaching another session cancels the watch.

For setup, logging, service management, and troubleshooting, see [Configuration and operations](development-and-operations.md).
