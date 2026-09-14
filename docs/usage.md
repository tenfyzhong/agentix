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
- `/sessions [codex|pi|omp|claude]` — list running sessions with their title, status, workspace, and terminal location; use an item's action to attach it
- `/rmux` or `/tmux` (selected by configuration) — browse terminal sessions, windows, and panes, or create a workspace and launch the selected agent
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
- `/goal [objective|pause|resume|clear]` — inspect or manage the thread goal; goal-driven turns show `/goal <objective>` under You in live, Background, attach, and history output when the local Codex rollout is available.
- `/review` — start an inline review of staged, unstaged, and untracked changes
- `/status` — show session, model, execution-policy, token-usage, goal details, and remaining account quota (window percentages, local reset times, and credits when reported)
- `/mcp` — list MCP server connection, authentication, and tool status

`/status` reads the [Codex account quota endpoint](https://learn.chatgpt.com/docs/app-server#6-rate-limits-chatgpt). A missing quota response is shown as unavailable or not reported; it does not hide other session details.

Pi and OMP expose `/model`, `/reasoning`, `/compact`, `/rename`, `/status`, `/skills`, `/diff`, and `/exit` according to the live host's capabilities. Model lists come from the host; reasoning changes report the level the host actually applies. They do not expose clear, fork, plan, goal, review, Fast mode, or MCP management. Third-party extension dialogs and approvals stay in the original terminal.

See the [bridge installation and capabilities](../plugins/agentix-bridge/README.md) for supported host versions, multiple-backend configuration, and diagnostics. Session IDs include their backend (`pi:<id>`, `omp:<id>`, `claude:<id>`, `codex:<id>`); bare IDs must resolve uniquely. Existing unqualified bindings require one startup with their original single-backend configuration before enabling multiple backends.

### Claude Code controls

Claude supports prompts, `/history`, `/status`, and `/queue` receipt recovery. `/detach` leaves Claude running. It does not expose remote stop, steering, model controls, or FIFO prompt submission; a busy session rejects another prompt. Handle permissions and interruptions locally. Default non-Channel input requires installing rmux or tmux and starting Claude inside it; [the Claude guide](claude-code.md) also documents optional Channel delivery.

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

Slack synchronizes an app-wide slash-command menu through Slack CLI at startup and publishes a contextual command reference in chat. See [initialization](slack-initialization.md). Telegram uses native command menus that change with attachment state. Attachment silently synchronizes native menus without posting command cards to Feishu or Slack; use `/help` to view the available commands. Feishu can present an interactive command card for other menu updates. Contextual commands use a `✌️` marker. `/attach` remains available as typed input, but the normal path is the Attach action returned by `/sessions`.

## Terminal workspaces

Set the global `[multiplexer]` table to `kind = "auto"` (default), `"rmux"`, or `"tmux"`. On startup, `auto` selects a responding native rmux service first, then a responding tmux command interface. Explicit `rmux` requires the native rmux protocol; explicit `tmux` accepts compatible services such as rmux. The service exposes only the successfully detected `/rmux` or `/tmux` command. If detection fails, neither command is registered and terminal management is disabled. Detection does not start either server: start one before Agentix, then restart Agentix to refresh detection. Choose an agent with, for example, `/tmux codex`, `/tmux pi`, `/tmux omp`, or `/tmux claude`. After creating an empty pane, choose the agent to launch. Browse existing sessions, windows and panes, attach an agent, or launch one in an idle shell pane. In the terminal browser, creation, split, refresh and back buttons use an accent color on Feishu and Slack; existing session/window/pane choices use the default color. Telegram currently renders these buttons without colors. New sessions, windows and splits preview the inferred directory before creating a pane. The removed `multiplexer.working_dir` setting is rejected; remove it from existing configuration files. Changing the global multiplexer settings requires restarting Agentix. tmux uses its local default server; the driver does not switch to rmux when tmux is unavailable.

On Unix, new sessions, windows and split panes start the user's interactive login shell (`-il`), overriding the multiplexer server's default shell/command. Exiting an empty shell closes its pane. Agents launched from IM run through the user's login shell with `-lc`, including shell configuration, exported environment, and functions (for example, a fish `claude` function). Agentix uses `AGENTIX_LOGIN_SHELL` when set, otherwise the current OS account's login shell; if account lookup fails, it falls back to `SHELL`, then `/bin/sh`. Commands and arguments remain literal values, not shell snippets. Use a command name such as `claude` to allow function lookup; an absolute executable path bypasses functions. Fish functions and environment setup must be available to non-interactive login shells, outside `status is-interactive` guards. When the agent exits, the pane returns to an interactive shell.

Creation does not depend on task boards or registered projects:

- A new session uses the attached agent’s current directory, or HOME when unavailable.
- A split uses its target pane’s directory, or HOME when unavailable.
- A new window defaults to HOME. The attached pane directory (or active pane directories when unattached) remains available as an explicit choice; it never replaces HOME automatically.

Use `Choose directory` to browse immediate subdirectories (six per page), show hidden directories, go to the parent or HOME, or `Enter path`. Paths may be absolute, start with `~/`, or be relative to the displayed directory. Spaces and Unicode are supported; shell variables and substitutions are not expanded. Symlinks resolve to their directory, including worktrees. Invalid manually selected paths show an error instead of switching to HOME.

`Create` revalidates the directory and terminal target, opens an interactive shell, and offers an agent picker. Selection applies only to this creation. `Cancel` or `/cancel` abandons the draft; other commands switch out of path input. Old controls expire after cancellation, another creation, reattachment or connection loss.

When the launched agent exits normally, fails, or is interrupted with Ctrl-C, the pane returns to an interactive shell. Exiting that shell with `exit` or Ctrl-D, or closing the pane, removes it. Agentix disables `remain-on-exit` locally on its panes, including empty panes before agent selection, so they do not remain as `Pane is dead`; global settings are unchanged.


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

`client sessions` emits normalized JSON for available sessions. With managed Codex, it includes running standalone and daemon-backed TUI sessions and their terminal locations while excluding stored sessions and orphaned daemon threads. `client call` sends raw Codex JSON RPC through the server's existing app-server connection for protocol diagnostics; JSON goes to stdout and logs go to stderr. `client claim` creates a temporary in-memory owner claim and is not an IM command.

The backend-neutral commands use the same access and capability policy as IM. Raw `client call` remains Codex-specific. See [local control and host protocol](host-protocol.md) for payloads and native extension interfaces.

## Restarts and process exits

During graceful shutdown, Agentix checkpoints bindings, removes live controls, silently restores detached native command menus, and sends an offline notice without stopping coding-agent sessions. On startup, it sends an online notice and reattaches sessions that remain available. Stale saved sessions are discarded and reported as detached. Neither startup nor shutdown posts command cards to Feishu or Slack; use `/help` to request commands.

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

In IM messages, consecutive reasoning items share one Reasoning block, and consecutive tool calls share one Tool Call block. Each item keeps its content and order; updates replace that item's content within the group. A change between reasoning, tool calls, and agent output starts a new block. Live turns, Background completion notifications, `/history`, and attach history use the same visibility switches and formatting. This applies to both collapsible cards and Markdown output. In Feishu, the newest process block is expanded while it is being output; once the next visible block appears, the previous process block collapses. Continuing an earlier answer also collapses the active process panel, even when the host reuses its original message ID. Late updates to an older tool only refresh its content. Agent output stays expanded. Older panels remain available to open manually. Pi/OMP native history process output requires an updated `agentix-bridge` plugin as well as Agentix; history responses retain the latest 20 process items per turn. Already received items are retained when a draining turn merges that bounded history.

Codex supplies completed reasoning summaries and tool items; Pi/OMP report completed thinking blocks and tool execution results. Claude imports visible thinking blocks and tool inputs/results from the matching transcript turn at completion. Content unavailable from the host is not reconstructed. Tool text may be shortened by the host bridge’s existing size limits. Standalone Taskix hooks retain their ordinary visible-message capture.

## Native new-session handoff

`/new` retains the IM conversation while the original session exits and its replacement starts. Pi uses its extension command context; Codex, OMP, and Claude require the original client inside rmux or tmux for IM initiation. Claude receives its native `/clear` command. Unsupported terminals report an error. Dismiss dialogs before requesting a switch. Codex and Claude terminal drafts are shown in IM for confirmation. Codex submission waits for `/new` to render stably before sending Enter, so rapid input is not treated as a pasted newline.

Starting a new session directly in the terminal also follows the same client (Claude: `/clear`), without requiring a multiplexer. Forks, ordinary resumes, and sessions in other clients are not handoffs. Client identity is independent of the working directory.

During the gap, plain messages enter a durable FIFO (up to 100 messages), then run one at a time after attachment. The wait expires after 30 seconds. A timeout deletes the handoff and cancels automatic attachment and queued delivery. Unsent queued text is returned in the timeout notice for manual resending. Subsequent prompts and `/new` requests are no longer blocked by the expired record. A switch still in progress reports its status without clearing a draft. `/queue` shows retained messages and failure state; `/queue clear` discards them. An uncertain delivery is retained and never automatically retried: inspect the destination history before clearing it. Manually attaching or detaching cancels automatic following and retains any old queue for inspection. Existing per-session queues remain with the old session.

Switch state, messages, binding epochs, and uncertain sends survive service restart and reload. The waiting notice is updated on completion or failure; attachment silently synchronizes command menus.


## Terminal draft confirmation

When Claude prompt delivery or Codex/Claude `/new` uses tmux/rmux, existing input is shown in IM before any clearing. Choose **Clear and send** to discard that exact draft and send the pending request, or **Cancel sending** to preserve it and cancel the request. If the draft changes before confirmation, Agentix shows the updated content and asks again. Finish this decision before sending another request; additional requests are rejected, and `/cancel` cancels the pending request. Changing attachment invalidates the old buttons.

Checks use the original client identity, pane ownership, input layout and the current draft. Dialogs or unreadable input prevent submission and report the reason in IM. Codex composers with the default terminal background are recognized from their prompt, padding and status footer; Vim normal/visual mode must be changed to insert mode first. Codex's ordinary prompts use its protocol and do not edit terminal input. Confirmation buttons are temporary: after a service restart, submit a direct request again; messages already in the `/new` handoff queue remain stored and request a fresh confirmation. No draft is cleared automatically. Collapsed paste/image placeholders must be expanded or handled in the terminal before their full contents can be reviewed. Terminal capture cannot recover text hidden by the host's own input viewport or undo a human keystroke racing the final check.
