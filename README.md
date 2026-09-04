# Agentix

Agentix connects the coding agents already running on your computer to the instant messaging apps you use when you step away. Tools such as OpenClaw and Hermes make agent interaction available through chat, but coding workflows usually begin in a terminal or IDE, where developers also inspect the code and review changes. Once you leave your computer, that local session becomes difficult to monitor or continue.

Agentix fills that gap without replacing your normal development workflow. It is a local-first Rust bridge that lets you check progress and send prompts to existing coding-agent sessions from Telegram or Feishu. It currently supports Codex, Pi, and Oh My Pi. Claude Code is intentionally out of scope for this release.

Each IM conversation is mapped explicitly and durably to an agent session. Live events are routed by their upstream session ID, and messages pair a readable session title with a short session ID and turn identifier so output from concurrent sessions remains unambiguous.

## Capabilities

- Codex native WebSocket-over-Unix-domain-socket transport, including managed-daemon auto-start, reconnect, subscription recovery, and bounded retry of interrupted attach/history reads across repeated reconnects
- Pi and Oh My Pi JSONL RPC subprocess transports, one isolated subprocess per attached session
- Telegram long polling and Feishu long connection
- Feishu Card JSON 2.0 and Telegram inline-button actions
- Owner allowlists, one-time Telegram and Feishu owner claiming, group mention requirements, event deduplication, and single-use action tokens
- Retryable inbound delivery with serialized IM/agent event handling
- Running-session discovery, attach/detach, recent history, prompting, persistent Codex follow-up queues, steering, stopping, approval decisions, and multi-question input round trips
- Interactive rmux workspace browsing and safe session, window, and pane creation from IM, with optional Codex launch and automatic attachment
- Attached Codex session controls for Fast mode, fresh sessions, Git diffs, renaming, compacting, forking, model/reasoning settings, skills, prompted plan mode, goals, reviews, detailed status, and MCP inventory
- Background turn-completion notifications that identify the session and provide a one-tap Attach action
- Automatic IM notification and temporary detachment when an attached Codex process exits, followed by reattachment when `codex resume` brings back the same session
- Local control client for listing available sessions, issuing raw Codex app-server requests through `serve`, and generating temporary owner claim codes
- Graceful serve lifecycle notifications with durable bindings, temporary IM detachment, and automatic reattachment after restart
- SQLite persistence for bindings, authoritative binding epochs, and active turn-message recovery after restart
- Concise per-turn IM rendering with separately labeled, quoted user input and Markdown agent output, including safe nested-quote handling and no tool execution details
- Telegram and Feishu reply context forwarded to the agent as a quoted block alongside the new prompt
- Telegram MarkdownV2 rendering for standard Markdown in agent replies, including streamed edits
- Telegram native command menus and Feishu interactive command cards that follow attachment state
- In-place stream updates with a one-second throttle, a per-second working timer, and a forced final flush

## Quick start

### macOS and Linux

On macOS and Linux, install Agentix with Homebrew:

```sh
brew tap tenfyzhong/tap
brew install agentix
mkdir -p ~/.config/agentix
cp "$(brew --prefix agentix)/share/agentix/agentix.example.toml" ~/.config/agentix/config.toml
```

After editing the configuration, run Agentix directly or start it as a Homebrew service:

```sh
agentix serve
# Or:
brew services start tenfyzhong/tap/agentix
```

### Windows (x86_64)

Download `agentix-<version>-x86_64-pc-windows-msvc.zip` and `SHA256SUMS` from the [latest GitHub release](https://github.com/tenfyzhong/agentix/releases/latest). Verify the archive checksum, then extract it in PowerShell:

```powershell
$archive = Get-ChildItem .\agentix-*-x86_64-pc-windows-msvc.zip | Select-Object -First 1
Get-FileHash $archive.FullName -Algorithm SHA256
Expand-Archive $archive.FullName -DestinationPath .\agentix
$env:Path = "$(Resolve-Path .\agentix);$env:Path"

New-Item -ItemType Directory -Force "$HOME\.config\agentix" | Out-Null
Copy-Item .\agentix\agentix.example.toml "$HOME\.config\agentix\config.toml"
```

Keep the extracted directory in a stable location and add it to your user `PATH` for future PowerShell sessions. Edit the copied configuration and select the Pi or Oh My Pi backend. The Codex backend is not available on Windows because its supported transport requires a Unix-domain socket.

Set the credentials for the selected channel, then validate the configuration and start Agentix:

```powershell
$env:AGENTIX_TELEGRAM_TOKEN = "..."
# Or, when channel.kind = "feishu":
$env:AGENTIX_FEISHU_APP_ID = "..."
$env:AGENTIX_FEISHU_APP_SECRET = "..."

agentix.exe doctor
agentix.exe serve
```

PowerShell environment assignments apply to the current session. Configure persistent user environment variables or a service manager when running Agentix in the background.

### Build from source

To build from source instead, install the Rust toolchain declared by `rust-toolchain.toml`, then run:

```sh
make release
cp config/agentix.example.toml ~/.config/agentix/config.toml
```

Set `[channel].kind` to exactly one active IM transport: `telegram` or `feishu`. Configure it under `[channel.telegram]` or `[channel.feishu]`. The inactive nested channel table may remain in the file, but Agentix neither validates nor starts it.

The Codex backend requires **Codex CLI 0.153.0 or newer installed by the official standalone installer**. On macOS or Linux, install or upgrade it with:

```sh
curl -fsSL https://chatgpt.com/codex/install.sh | sh
```

Installing **Codex** through Homebrew is not supported: that Codex package does not provide the managed standalone app-server layout that Agentix needs. Confirm that the official standalone Codex binary and its app-server daemon are available before starting Agentix:

```sh
~/.codex/packages/standalone/current/codex --version
~/.codex/packages/standalone/current/codex app-server daemon version
```

Agentix automatically starts the shared local daemon when the managed `unix://` socket is unavailable. The equivalent manual command is:

```sh
~/.codex/packages/standalone/current/codex app-server daemon start
```

Set `agent.command = "~/.codex/packages/standalone/current/codex"` so Agentix always uses that supported installation, especially when it runs with a restricted service `PATH`. A leading `~/` is expanded to the current user's home directory. The same expansion applies to all filesystem paths in the configuration and to custom Codex endpoints such as `unix://~/.codex/custom.sock`. Set `agent.rmux_directory` to choose the workspace used when `/rmux` creates a session, window, or pane; it defaults to the current user's Home directory. The former `agent.multiplexer_directory` key remains accepted as a compatibility alias.

Export only the secrets for the selected channel:

```sh
export AGENTIX_TELEGRAM_TOKEN='...'
# Or, when channel.kind = "feishu":
export AGENTIX_FEISHU_APP_ID='...'
export AGENTIX_FEISHU_APP_SECRET='...'
```

For a new Telegram or Feishu installation, omit the selected channel's owner list or leave it empty (`channel.telegram.owner_user_ids` or `channel.feishu.owner_open_ids`). Start `agentix serve`, generate a temporary code from another local terminal, then send the printed command to the bot in a private chat:

```sh
agentix client claim
# /claim 12AB34CD56EF
```

The code is printed only to that terminal and is valid for 10 minutes by default. Use `--ttl-minutes <1-1440>` to change its lifetime. `serve` generates and retains the pending code only in memory, so restarting it invalidates the code and no claim secret or expiry is written to disk. A successful claim immediately authorizes the account, atomically adds its numeric Telegram user ID or Feishu `open_id` to the same config file, and consumes the in-memory code. Existing non-empty owner lists cannot generate a claim code.

Feishu reply context uses the official get-message API. Grant the application either `im:message` or `im:message:readonly`; quoting messages in group chats additionally requires `im:message.group_msg`. If the original message cannot be read, Agentix still forwards the new prompt without quoted context. When Feishu rejects a request with `99991663`, Agentix invalidates the cached tenant access token, obtains a fresh token, and retries that request exactly once. A second rejection is returned without another retry, preventing repeated message delivery.

Validate the installation and run the bridge:

```sh
cargo run -p agentix -- doctor
cargo run -p agentix -- serve
```

Without `--config`, Agentix reads `$HOME/.config/agentix/config.toml`. Pass `--config <file>` before the subcommand to use another file.

Logging defaults to `info` on stderr. Configure the tracing filter with `[logging].level`; `RUST_LOG` takes precedence when set. Optional file logging supports time-based rotation and bounded retention:

```toml
[logging]
level = "agentix=debug,agentix_codex=info"

[logging.file]
enabled = true
path = "~/.local/state/agentix/agentix.log"
rotation = "daily" # never, minutely, hourly, or daily
max_files = 7
```

Parent directories are created automatically, file output omits ANSI escape sequences, and timestamps use the computer's local time zone on both stderr and file output.

For local diagnostics, use the CLI client while `agentix serve` is running. On macOS and Linux, `serve` listens on `unix://~/.local/share/agentix/control.sock` by default; on Windows, it listens on `tcp://127.0.0.1:32198`. Override this with `[server].endpoint`. TCP endpoints must use a numeric loopback address. The Unix socket is created with owner-only permissions and removed during graceful shutdown.

Every `agentix client` subcommand connects to that Agentix control endpoint rather than opening another connection to the coding-agent backend. `sessions` prints the sessions currently available from the configured agent as JSON. With the managed Codex endpoint, Agentix correlates interactive Codex processes with their writer locks and daemon-owned threads. This includes running standalone and daemon-backed TUI sessions while excluding stored sessions and orphaned daemon threads. When a Codex process runs inside rmux, Agentix also reports its rmux session, window index and name, and pane index and ID.

```sh
agentix client sessions
agentix client sessions --limit 10
agentix client call thread/read --params '{"threadId":"019...","includeTurns":false}'
agentix client claim --ttl-minutes 10
```

`client call` asks the running server to issue raw Codex JSON RPC for protocol debugging. Its stdout is JSON; logs are written to stderr with RFC 3339 timestamps in the computer's local time zone, so the result can be piped to tools such as `jq`. `client claim` asks the running server to create an in-memory claim and is not registered or parsed as a remote IM command.

## IM commands

- `/help` — show the commands currently available to the conversation
- `/sessions` — list running sessions as numbered, framed title/status/workspace items with Home-relative `~` paths and rmux locations; neutral attach buttons repeat each item's sequence number, while the current session is marked as attached and omitted from the buttons
- `/rmux` — browse rmux by session, window, and pane; create a default-named session or window in `agent.rmux_directory`, split the active pane there, launch Codex immediately, and attach it automatically
- `/attach <session-id>` — bind this conversation to an existing running session, resume paginated Codex threads without full-history hydration, fetch only the latest turn, and restore a Stop action when that turn is running; a loaded session awaiting its first user message is attached provisionally and resumed after that message materializes it, while attaching the already-current session only returns a short notice
- `/current` — show the current session and running turn
- `/history`, `/history older`, `/history newer` — inspect history with separate user and agent sections for each turn
- `/queue` — inspect the attached Codex session's persistent FIFO follow-up queue
- `/stop` — interrupt the current turn
- `/detach` — remove the current binding
- `/cancel` — leave a pending free-text input flow
- any other text — start an idle turn; while Codex is active, enqueue a separate follow-up turn and immediately report its position; backends without persistent queues continue to steer

Codex CLI's Tab queue and Agentix's app-server queue are separate and do not synchronize or deduplicate each other. If both queues contain input when a turn finishes, both queue owners may try to submit their next item, so messages can be submitted back-to-back and their relative order is not guaranteed. Avoid using both queue mechanisms concurrently for the same session.

An unknown or malformed slash command is answered in IM with its parsing error and the same state-aware command list shown by `/help`. The inbound event is then marked as handled, so it is neither logged as a failed request nor retried.

The following Codex-native commands are available only while the IM conversation is attached:

- `/fast [on|off]` — toggle the current model's Fast service tier, when advertised by Codex
- `/clear [name]` — start and attach a fresh session with the current directory, model, approval, sandbox, and service-tier settings; disabled while a turn is active
- `/exit` — leave the attached session from IM without stopping Codex
- `/diff` — show staged, unstaged, and untracked Git changes in the attached workspace
- `/rename [name]` — rename the attached Codex session; without a name, the next IM message supplies it
- `/compact` — start context compaction
- `/fork` — fork the attached session and attach the fork
- `/model [model-id]` — show the current and available models with buttons, or select one for subsequent turns
- `/reasoning [effort]` — show the current and supported reasoning efforts with buttons, or select one for subsequent turns
- `/skills` — list enabled skills available in the attached workspace
- `/plan [prompt]`, `/plan off` — enter plan mode and optionally start a turn with the inline prompt, or return to default mode; mode changes are disabled while a turn is active
- `/goal [objective|pause|resume|clear]` — inspect or manage the thread goal
- `/review` — start an inline review of staged, unstaged, and untracked changes
- `/status` — show the thread name and ID, state, directory, model, reasoning and service tier, approval policy, sandbox, writable roots, live token/context usage when reported, and goal
- `/mcp` — list MCP server connection, authentication, and tool status

In groups, the bot must be mentioned. Direct messages are accepted only from configured owners, except for an unexpired `/claim <code>` match when the selected Telegram or Feishu owner list is empty. Claim attempts in groups are ignored. A session can be current in only one IM conversation at a time. These extended commands are currently implemented by the Codex adapter; other agents reject them as unsupported.

`/rmux` is backed exclusively by the official `rmux-sdk` crate. Opening it connects to the local rmux daemon and starts that daemon when needed. Existing Codex panes can be attached directly, and idle shell panes can start a new Codex session. Creating a session, window, or split is a single action: Agentix uses `codex` as the default name, resolves session-name conflicts with a numeric suffix, and uses `agent.rmux_directory` as the workspace. Session, window, pane, process, and foreground-state operations use typed rmux SDK requests; Agentix never invokes the tmux-compatible CLI. Before replacing an existing shell pane, Agentix sends `Ctrl-C` through the SDK to discard any unsubmitted command line, then launches Codex as structured argv connected to the configured app-server. Fresh sessions, windows, and splits skip that input step. The rmux pane is configured to remain visible after Codex exits. Agentix waits for the real session to appear in the created pane, then binds the IM conversation without trying to resume the still-empty rollout. The first IM turn creates the rollout, after which Agentix rejoins the running thread and receives its full event stream. If Codex exits before becoming ready or session discovery times out, the action returns an explicit error instead of attaching a placeholder session. If the directory is not configured, it defaults to Home. Agentix refuses to replace a pane running a non-shell process.

When the Telegram channel starts, Agentix registers a minimal native menu and selects the commands menu button for private chats. Attaching a Codex session replaces that chat's menu with the attached-session commands, whose descriptions use a `✌️` marker to distinguish them from commands that are always available; manually detaching or observing the Codex process exit restores the minimal menu. A process exit only suspends the binding: resuming the same Codex session restores the attached menu automatically. Feishu does not expose a runtime API for per-conversation native bot menus, so Agentix sends an interactive command card after attachment and edits the same card as attachment state changes. Its contextual buttons use the same `✌️` marker. Persisted attachments restore their extended menus after an Agentix restart. `/attach` remains available as a typed command, but attachment is normally performed through the title buttons returned by `/sessions`.

Session-specific IM output uses `title · short-id` rather than an ID alone. This applies to `/current`, history and live-turn headers, command results, queue views, background completion notices, manual detach notices, automatic process-exit detachment, and serve restart notifications.

An IM prompt immediately creates its turn message with `Working 0s`. While that attached turn remains active, Agentix edits the same message once per second even when Codex emits no text, advances the working duration, and preserves the existing Stop action. Completion, interruption, or failure stops the timer and leaves the final elapsed duration in the status line. A running turn restored after an Agentix restart starts a new locally observed duration because the agent protocol does not expose its original monotonic start time.

When a turn finishes in a session that is not attached to an IM conversation, Agentix sends the authenticated conversations seen by the running service one notification containing that session's title and short ID. The notification includes a single-use Attach button. If the session was previously attached and is draining after a switch, Agentix updates its existing turn card and replaces the completed controls with the same Attach action. Replayed completion events do not create duplicate notifications, and the currently attached session continues to use its normal turn card without an additional notice.

After Agentix validates and consumes a button action, its source controls are immediately made non-interactive: Telegram removes the inline keyboard, while Feishu keeps the buttons visible but disabled. This prevents repeated clicks while the selected operation is still completing.

Codex approval and plan-input requests are rendered as separate actionable messages. Approval buttons are removed after a Telegram decision and the selected result is shown in place. Multi-question input is presented one question at a time with option buttons and an `Other…` free-text path; Agentix submits all answers together and then replaces the controls with the answer summary. If the same request is resolved in Codex CLI, Agentix removes its Telegram controls and marks it as resolved outside Telegram because the app-server notification does not include the selected decision or answers.

Replying to a Telegram or Feishu message adds that message as quoted context before the new prompt sent to the attached agent. Only the earlier message is blockquoted; the new input remains the ordinary prompt text. A manually selected Telegram quote takes precedence over the full replied-to text or media caption. Feishu resolves the parent message through OpenAPI and extracts visible text from text, rich-text, and card messages. Slash commands ignore reply context so command parsing remains unambiguous.

On graceful shutdown, `agentix serve` checkpoints its durable bindings, removes live controls, restores each bound IM conversation's detached menu, and sends an offline notification without stopping the coding-agent session. On startup, it notifies those conversations that the service is online and reattaches sessions that are still running. A saved session that is no longer attachable is discarded and reported as detached.

## Design documents

- [Contributing](CONTRIBUTING.md)
- [Product design](docs/product-design.md)
- [Architecture](docs/architecture.md)
- [Development and operations](docs/development-and-operations.md)

## Development

The workspace uses TDD and treats warnings as defects during CI:

```sh
make check
```

GitHub Actions keeps linting in `ci.yml`. The `tests.yml` workflow runs the full suite on Linux and macOS, and checks the workspace plus the native TCP control suite on Windows. It runs for pull requests and pushes to `main`, and it can also be started manually with `workflow_dispatch`.

Pushing a `v<version>` tag starts the `Release` workflow. The tag version must exactly match `[workspace.package].version` in `Cargo.toml`; a mismatch stops the release before any artifact is published. The workflow builds native archives for macOS arm64, Linux x86_64/arm64, and Windows x86_64, verifies each binary's `--version`, then publishes checksums and a GitHub Release. After the release is published, the workflow builds a Homebrew bottle, uploads it to the release, and opens or updates the formula PR in `tenfyzhong/homebrew-tap`. Homebrew publishing requires the `HOMEBREW_TAP_TOKEN` repository secret.

Run `make` for a debug build or `make help` to list the available targets.

The main crates are `agentix-core`, `agentix-codex`, `agentix-pi`, `agentix-telegram`, `agentix-feishu`, and the `agentix` executable.

The core exposes a small common agent interface plus optional queue, attached-session control, and workspace-runtime ports. A serialized runtime loop feeds IM and agent events into coordinator-owned session, turn, interaction, and rmux state. See the [architecture document](docs/architecture.md) for the state/effect and retry boundaries.

The test suite is layered. Protocol and rendering tests cover pure mappings; adapter tests cover Telegram, Feishu, Pi, and Codex transport behavior; core tests exercise routing, persistence, actions, interactions, and lifecycle transitions. Codex has a stateful mock app-server in `crates/agentix-codex/tests/support/`. Its protocol contract tracks the Codex CLI 0.153.0 schema for the complete subset used by Agentix, including thread creation and naming, settings and service tiers, goals, token usage, lifecycle and tool events, approvals, user-input questions, pagination, injected RPC failures, and reconnect/resubscribe. Full-stack tests pass real Telegram and Feishu inbound events through their adapters, the engine, and the Codex client, then verify the resulting answer at the mocked channel API. The mock uses the real WebSocket-over-UDS transport but never requires a locally installed or running Codex daemon.

Telegram tests use an in-process mock Bot API for startup discovery, command-menu registration, long polling, authorization filtering, owner claiming, callback acknowledgement, sends, edits, cleared keyboards, reply context, and API failures. Feishu tests use an in-process mock OpenAPI and WebSocket service for tenant tokens, bot discovery, interactive-card sends and updates, dynamic command cards, message lookup and reply context, message events, card actions, owner claiming, acknowledgements, and API failures. Its token mock issues a different credential on every authorization request and verifies one-shot `99991663` recovery for sends, card edits, action cleanup, command-menu sends and edits, reply lookup, and claim responses. It also verifies that ordinary API errors do not refresh, a failed refresh does not replay a message, and a second invalid-token response stops retrying. Binary CLI tests execute `agentix client sessions`, `agentix client call`, `agentix client claim`, and `agentix doctor` against local fixtures, including file-log rotation, retention, local timestamps, and ANSI-free output. Service lifecycle tests exercise durable restart reattachment, shutdown notification, control-listener orchestration, and the bounded channel shutdown deadline. CI runs all platform-applicable tests on Linux, macOS, and Windows; the TCP control suite therefore runs natively on every platform. rmux tests put typed SDK packets over a real Unix socket to a mock daemon for pane clearing, process launch, and session, window, and split creation. Pi uses a reusable fake RPC subprocess. These fixtures keep the integration suite deterministic and free of live credentials or network services.
