# Using Agentix

Agentix exposes local coding-agent sessions through Telegram, Feishu, or Slack. Start `agentix serve`, open the configured bot, and use `/sessions` to find a running session. Select its Attach action, then send ordinary chat messages to prompt the agent.

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
- `/sessions [codex|pi|omp|claude]` — list running sessions with their title, status, workspace, and rmux location; use an item's action to attach it
- `/rmux` — browse rmux sessions, windows, and panes, or create a workspace and launch the selected agent
- `/attach <session-id>` — attach the conversation to a running session
- `/current` — show the attached session and running turn
- `/history`, `/history older`, `/history newer` — browse turns with separate user and agent sections
- `/queue` — inspect a supported follow-up queue, or unresolved delivery receipts for Claude
- `/stop` — interrupt the current turn when supported (Codex, Pi, OMP)
- `/detach` — remove the current binding without stopping the coding-agent session
- `/cancel` — leave a pending free-text input flow
- any other text — start a turn; while a queue-capable agent is active, add a follow-up turn to its Agentix queue; use `/steer <text>` when steering is supported

Unknown or malformed slash commands return a parsing error and the same state-aware command list shown by `/help`.

### Optional task commands

When `task_board.enable = true` and `[task_board].config` points to a taskix configuration, `/dashboard` lists projects with actions to open their boards. After attachment, `/board` shows the current session's task board and `/jobs` lists its associated Jobs. Select a Task or Job to read its Markdown details; Job details link to associated Tasks, and Task details include a **Job** button. `task_board.enable` defaults to `false`; when disabled, the task board is not started and its commands are omitted from IM menus and help. Legacy `/tasks [job-or-project]` and `/task <id>` remain typed shortcuts and are not added to command menus.

An attached session can claim a Task and use state actions; Block, Wait, and Fail request a reason, and `/cancel` clears that input. The agent publishes a Plan through taskix before Start is allowed. Done is offered only during execution. IM does not create Jobs/Tasks or edit Plan bodies. See the [task board workflow](task-board.md#agentix-integration).

### Human job submissions

After attaching a session in a registered Project, use `/inboxes` to browse its human queue. Submit one requirement with `/inbox`:

```text
/inbox Add CSV export

Preserve the current filter and column order.
- [ ] Include a header row
- [ ] Cover Unicode values
```

The entire message becomes one `TODO` entry at the end of the Project Inbox. The reply links to its details. After reviewing the agent’s current result, explicitly ask it to take the next Job from the Inbox. Each request takes one eligible entry through the normal task workflow; completion returns control to you and does not start another Job. Submission also works with a read-only attachment and does not start an agent turn.

You can also edit the Project's `Inbox.md`: use `- [-]` to cancel, or delete an unfinished entry to withdraw it. See [Project inbox](task-board.md#project-inbox) for leases, recovery, and cancellation rules. `/cancel` still cancels pending IM input; it does not cancel an Inbox entry.

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
- `/status` — show session, model, execution-policy, token-usage, goal details, and remaining account quota (window percentages, local reset times, and credits when reported)
- `/mcp` — list MCP server connection, authentication, and tool status

`/status` reads the [Codex account quota endpoint](https://learn.chatgpt.com/docs/app-server#6-rate-limits-chatgpt). A missing quota response is shown as unavailable or not reported; it does not hide other session details.

Pi and OMP expose `/model`, `/reasoning`, `/compact`, `/rename`, `/status`, `/skills`, `/diff`, and `/exit` according to the live host's capabilities. Model lists come from the host; reasoning changes report the level the host actually applies. They do not expose clear, fork, plan, goal, review, Fast mode, or MCP management. Third-party extension dialogs and approvals stay in the original terminal.

See the [bridge installation and capabilities](../plugins/agentix-bridge/README.md) for supported host versions, multiple-backend configuration, and diagnostics. Session IDs include their backend (`pi:<id>`, `omp:<id>`, `claude:<id>`, `codex:<id>`); bare IDs must resolve uniquely. Existing unqualified bindings require one startup with their original single-backend configuration before enabling multiple backends.

### Claude Code controls

Claude supports prompts, `/history`, `/status`, and `/queue` receipt recovery. `/detach` leaves Claude running. It does not expose remote stop, steering, model controls, or FIFO prompt submission; a busy session rejects another prompt. Handle permissions and interruptions locally. Default non-Channel input requires installing rmux and starting Claude inside rmux; [the Claude guide](claude-code.md) also documents optional Channel delivery.

## Prompts, queues, and replies

An ordinary message starts a turn when the attached session is idle. While Codex is active, Agentix places new messages in its persistent app-server FIFO queue and immediately reports their positions. `/queue` shows this queue.

For Pi/OMP, `/stop` also pauses pending work. `/queue resume` continues it, while `/queue clear` discards pending items without stopping an active delivery. The extension persists queue state and request IDs in the native session log. Reloading never silently repeats an uncertain delivery; inspect history before clearing and submitting again. Queue entries do not transfer into forks. `/steer <text>` explicitly injects text into the active turn.

Codex CLI's Tab queue is private to the TUI and does not synchronize or deduplicate with the Agentix app-server queue. If both contain input, each queue may submit its next item when a turn finishes, producing back-to-back turns with no shared ordering guarantee. Avoid using both queue mechanisms for the same session at the same time.

Replying to a Telegram or Feishu message adds the earlier message as quoted context before the new prompt. A manually selected Telegram quote takes precedence over the full replied-to text or media caption. Feishu extracts visible content from text, rich-text, and card messages. Slash commands ignore reply context.

## Interactive requests

Agentix renders Codex approval and plan-input requests as separate actionable messages. After an approval decision, Agentix removes or disables the original controls and shows the result in place.

For multi-question input, Agentix presents one question at a time with option buttons and an `Other…` free-text path. It submits all answers together and replaces the controls with an answer summary. If the request is resolved in Codex CLI, Agentix marks the IM request as resolved outside the chat because the app-server notification does not include the selected decision or answers.

Slack uses native `/agentix-sessions` commands synchronized at startup, as well as `/agentix /sessions` and other `/agentix <command>` invocations, or bot mentions inside threads; see [Slack usage and setup](slack.md).

Every action token is single-use. Telegram and Slack remove consumed controls; Feishu leaves the buttons visible but disabled.

## Session and message behavior

Session-specific output uses `title · short-id` in current-session views, history, live-turn headers, command results, queue views, completion notices, and lifecycle notices.

If another Codex process already owns a session's writer, selecting it connects read-only and displays the latest saved turn. Agentix checks for updated content every ten seconds, including when background notifications are disabled. `/history` requests the latest saved content immediately. The menu offers viewing and navigation commands, and attempts to send prompts, stop turns, or change settings display a read-only explanation. Use the original Codex process for those operations. This connection does not subscribe to its live event stream, so updates depend on when Codex saves its history. `/sessions` infers activity from the latest saved turn when the connected app-server reports `notLoaded`.

Other attachment errors appear in IM with their cause and a new Retry attach button. The previous binding remains available after a failed selection. A read-only connection stays read-only until detached; after the original writer releases the session, detach and select it again to try a writable attachment.

Agentix creates a turn message immediately with `Working 0s`, edits it at most once every five seconds on Telegram (once per second on Feishu and every two seconds on Slack) while the turn runs, and preserves its Stop action. Completion, interruption, or failure leaves the final elapsed time in the status line. After an Agentix restart, a restored running turn begins a new locally observed duration because the agent protocol does not expose its original monotonic start time.

When a turn finishes in a session that is not attached to an IM conversation, Agentix notifies authenticated conversations known to the running service and includes the completed turn's prompt and response plus a single-use Attach action. Background notices use a purple Feishu header and a grey quote area; Telegram uses a ⚫ Background marker with blockquotes because its Markdown message format cannot set quote colors. Codex subagent sessions do not generate these standalone notices; parent sessions and existing attached or draining turn cards continue updating normally. Repeated delivery of the latest completed turn does not create duplicate notices. For Codex, the service discovers running sessions and reads their turn status every ten seconds, even when no IM conversation is attached. `/detach` removes the IM binding; a running session is rediscovered for background notifications. Background monitoring uses read-only queries and leaves existing writers alone. Historical completions from before service startup are skipped, and completed, failed, or interrupted turns detected between polls are reported once. Send `/help` to the bot after starting the service if you have no restored binding and want to receive these notifications.

Set `[notifications] background_turns = false` in `config.toml` and restart the service to disable unattached completion notices and background turn polling. With no attached sessions to monitor, automatic session discovery also stops. Attached-session exit/resume monitoring remains active. The setting defaults to `true`; existing attached and draining turn cards continue updating to their final status. When enabled, the engine reads the completed turn by ID so a newer turn cannot replace its content. If history is unavailable, the completion notice still shows its outcome and Attach action.

Slack synchronizes an app-wide slash-command menu through Slack CLI at startup and publishes a contextual command reference in chat. See [initialization](slack-initialization.md). Telegram uses native command menus that change with attachment state. Feishu sends an interactive command card and updates it as the state changes. Contextual commands use a `✌️` marker. `/attach` remains available as typed input, but the normal path is the Attach action returned by `/sessions`.

## rmux workspaces

`/rmux` connects to the local rmux daemon and starts it when needed. Choose a backend with `/rmux codex`, `/rmux pi`, or `/rmux omp`. An unbound chat with several configured backends gets a picker. You can attach an existing agent pane or replace an idle shell pane with a new session. Agentix can also create a session, window, or split in `agent.rmux_directory`, which defaults to the current user's home directory.

Before replacing an existing shell pane, Agentix sends `Ctrl-C` to discard any unsubmitted command line. It refuses to replace a pane running a non-shell process. New panes remain visible after the agent exits. Agentix waits for the real session before attaching. Pi/OMP must register a live bridge on the Agentix control socket associated with the new pane; if registration fails or times out, Agentix reports the error and leaves the terminal open. Configure `bridge_extension` when the host does not already discover the installed extension.

## Local client

Every `agentix client` command connects to the control endpoint of the running `agentix serve` process. The defaults are `unix://~/.local/share/agentix/control.sock` on macOS and Linux, and `tcp://127.0.0.1:32198` on Windows.

```sh
agentix client sessions
agentix client sessions --limit 10
agentix client send pi:native-id "Continue the task"
agentix client stop omp:native-id turn-id
agentix client history pi:native-id --limit 20
agentix client command omp:native-id '{"command":"model","params":null}'
agentix client call thread/read --params '{"threadId":"019...","includeTurns":false}'
agentix client claim --ttl-minutes 10
```

`client sessions` emits normalized JSON for available sessions. With managed Codex, it includes running standalone and daemon-backed TUI sessions and their rmux locations while excluding stored sessions and orphaned daemon threads. `client call` sends raw Codex JSON RPC through the server's existing app-server connection for protocol diagnostics; JSON goes to stdout and logs go to stderr. `client claim` creates a temporary in-memory owner claim and is not an IM command.

The backend-neutral commands use the same access and capability policy as IM. Raw `client call` remains Codex-specific. See [local control and host protocol](host-protocol.md) for payloads and native extension interfaces.

## Restarts and process exits

During graceful shutdown, Agentix checkpoints bindings, removes live controls, restores detached command menus, and sends an offline notice without stopping coding-agent sessions. On startup, it sends an online notice and reattaches sessions that remain available. Stale saved sessions are discarded and reported as detached.

With managed Codex, exiting an attached Codex process temporarily detaches the IM conversation and starts watching for the same session ID. Running `codex resume` for that session restores the app-server subscription and binding automatically. Manually detaching or attaching another session cancels the watch.

For setup, logging, service management, and troubleshooting, see [Configuration and operations](development-and-operations.md).

## Changing the configured bot

Before restoring bindings, Agentix compares the configured bot with the identity stored in SQLite. This applies to Telegram, Feishu, and Slack:

| Channel | Stable identity |
| --- | --- |
| Telegram | Bot user ID returned by `getMe` |
| Feishu | Configured App ID |
| Slack | Workspace ID and bot user ID returned by `auth.test` |

Restarting with the same bot preserves bindings. Rotating its token or app secret also preserves bindings when its identity stays the same. If a remote identity lookup fails, startup fails before identity reconciliation changes any channel's state; fix authentication or connectivity and restart.

Changing bots clears that channel's old bindings, saved message views, pending interactions, event deduplication records, and queued notifications in one transaction. Binding epochs remain monotonic, and notification cursors are retained. Other channels and upstream agent sessions are preserved. Switching back to the previous bot does not revive its old routes. Open the new bot's conversation, run `/sessions`, and attach the desired session again.

Old IM routing data without a stored bot identity is unsupported in both development and released versions. On startup, Agentix deletes that data for each configured channel instead of migrating it or adopting it for the current bot. Use `/sessions` to reattach. No manual database edits or identity configuration are needed.

## Process output

In the Agentix configuration, enable either independent option:

```toml
[output]
show_reasoning = true
show_tool_calls = true
```

Both default to `false`. Run `agentix reload` after changing them. Enabled host-exposed reasoning summaries and tool details appear alongside the final answer and are written into the associated Obsidian Job’s Agent output when the turn ends, including sessions without an IM attachment. This does not change the model’s reasoning level.

In IM messages, consecutive reasoning items share one Reasoning block, and consecutive tool calls share one Tool Call block. Each item keeps its content and order; updates replace that item's content within the group. A change between reasoning and tool calls starts a new block. This applies to both collapsible cards and Markdown output.

Codex supplies completed reasoning summaries and tool items; Pi/OMP report completed thinking blocks and tool execution results. Claude imports visible thinking blocks and tool inputs/results from the matching transcript turn at completion. Content unavailable from the host is not reconstructed. Tool text may be shortened by the host bridge’s existing size limits. Standalone Taskix hooks retain their ordinary visible-message capture.
