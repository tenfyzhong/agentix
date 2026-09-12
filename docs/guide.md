# Agentix detailed guide

For a short installation and first-session walkthrough, start with the [README](../README.md). This guide covers installation alternatives, shell completions, configuration, session behavior, and optional task boards.

- [Installation and setup](#installation-and-setup)
- [Optional rmux dependency](#rmux-optional)
- [Shell completions](#shell-completions)
- [Configuration](#configure)
- [Starting the service](#start)
- [Capabilities and session behavior](#capabilities-and-session-behavior)
- [IM task boards](#im-task-boards)
- [Development and contributing](../CONTRIBUTING.md)
- [Further documentation](#documentation)

Agentix connects the coding agents already running on your computer to Telegram, Feishu, or Slack, so you can monitor and continue local sessions when you step away from the terminal or IDE. It is a local-first Rust bridge for Codex, Pi, Oh My Pi, and Claude Code. Claude Code uses the [plugin with rmux input](claude-code.md).

Each IM conversation maps explicitly and durably to an agent session. Sessions without a name or prompt preview use the agent name and the last segment of the native session ID, for example `Codex · 06423a09a510`. Messages include a readable session title, short session ID, and turn identifier so concurrent sessions remain unambiguous.

## Installation and setup

### Install

#### macOS and Linux

Install Agentix with Homebrew:

```sh
brew tap tenfyzhong/tap
brew install agentix
```

The Homebrew formulae for Agentix and the standalone task manager, taskix, are maintained in [tenfyzhong/homebrew-tap](https://github.com/tenfyzhong/homebrew-tap). Release automation updates both formulae and publishes macOS arm64, Linux x86_64, and Linux arm64 bottles. All platforms build the same prepared formula; after every bottle build succeeds, automation merges their metadata into one pull request per formula. Install taskix with `brew install tenfyzhong/tap/taskix`. The Homebrew workflow can also be run manually for an existing release tag, selecting `agentix`, `taskix`, or `all` (the default) to publish the corresponding bottles and formula pull requests.

The Codex backend requires Codex CLI 0.153.0 or newer from the official standalone installer. The Homebrew Codex package does not include the managed app-server layout Agentix needs.

```sh
curl -fsSL https://chatgpt.com/codex/install.sh | sh
```

#### Windows (x86_64)

Download `agentix-<version>-x86_64-pc-windows-msvc.zip` and `SHA256SUMS` from the [latest GitHub release](https://github.com/tenfyzhong/agentix/releases/latest). Verify and extract the archive in PowerShell:

```powershell
$archive = Get-ChildItem .\agentix-*-x86_64-pc-windows-msvc.zip | Select-Object -First 1
Get-FileHash $archive.FullName -Algorithm SHA256
Expand-Archive $archive.FullName -DestinationPath .\agentix
$env:Path = "$(Resolve-Path .\agentix);$env:Path"
```

Keep the extracted directory in a stable location and add it to your user `PATH`. The current native IM backends require macOS/Linux and a Unix socket. Windows packages remain useful for standalone taskix and task plugins; they do not provide native IM bridging for Codex, Pi, OMP, or Claude.

#### Separate release archives

Each [GitHub release](https://github.com/tenfyzhong/agentix/releases/latest) publishes Agentix and taskix separately, using the same version and targets (macOS arm64, Linux x86_64/arm64, and Windows x86_64):

- `agentix-<version>-<target>.tar.gz`: Agentix, its example configuration, and its shell completions.
- `taskix-<version>-<target>.tar.gz`: taskix, its example configuration, its shell completions, task documentation, and `plugins/taskix-manager/`.

Both archives include `README.md` and `LICENSE`. Windows also has `.zip` archives for each tool. The shared `SHA256SUMS` covers both tools and all archive formats. Verify the downloaded archive against its matching checksum before extracting it, then add the extracted binary's directory to `PATH`.

Download the taskix archive to use the task board independently; Agentix is not required. Download both archives if you need both tools. Keep the taskix plugin directory at a stable path and follow the [plugin activation guide](../plugins/taskix-manager/README.md).

#### Build from source

Install the Rust toolchain declared by `rust-toolchain.toml`, then run:

```sh
make release
```

The binaries are written to `target/release/agentix` and `target/release/taskix` (`.exe` on Windows).

### Terminal multiplexer (optional)

Install rmux or tmux to create new agent sessions from Telegram, Feishu, or Slack. Set `multiplexer.kind` to `rmux` (default) or `tmux`, then send the matching `/rmux` or `/tmux` command to browse workspaces and launch an agent in a session, window, or split. Set the shared `multiplexer.working_dir` for new workspaces; it defaults to `"~"`. A multiplexer is optional for connecting to existing Codex sessions.

See [terminal workspaces](usage.md#terminal-workspaces) for details.

### Shell completions

Both CLIs support `completions bash`, `completions zsh`, and `completions fish`.
For example, use `agentix completions bash` or `taskix completions bash`.
Generation requires no configuration, running server, or task database.

For bash, add this line to `~/.bashrc` (or `~/.bash_profile` on macOS):

```bash
source <(agentix completions bash)
source <(taskix completions bash)
```

For zsh, save the completion file:

```zsh
mkdir -p ~/.zsh/completions
agentix completions zsh > ~/.zsh/completions/_agentix
taskix completions zsh > ~/.zsh/completions/_taskix
```

Add the following to `~/.zshrc`, placing the `fpath` line before any existing
`compinit` call. If your shell framework already calls `compinit`, use that call
instead of adding another one:

```zsh
fpath=(~/.zsh/completions $fpath)
autoload -Uz compinit
compinit
```

For fish:

```fish
mkdir -p ~/.config/fish/completions
agentix completions fish > ~/.config/fish/completions/agentix.fish
taskix completions fish > ~/.config/fish/completions/taskix.fish
```

Restart your shell after installation. Regenerate saved files after upgrading
either CLI. Source checkouts include ready-to-use files
in `completions/`: `agentix.bash`, `_agentix`, `agentix.fish`, `taskix.bash`,
`_taskix`, and `taskix.fish`. Each release archive includes only its own CLI's
three completion files. Enable only the CLIs you have installed. You can
source the bash files or copy the zsh/fish files to the directories above.

### Configure

Create the default configuration file.

For a Homebrew installation:

```sh
mkdir -p ~/.config/agentix
cp "$(brew --prefix agentix)/share/agentix/agentix.example.toml" ~/.config/agentix/config.toml
```

For a source checkout:

```sh
mkdir -p ~/.config/agentix
cp config/agentix.example.toml ~/.config/agentix/config.toml
```

On Windows:

```powershell
New-Item -ItemType Directory -Force "$HOME\.config\agentix" | Out-Null
Copy-Item .\agentix\agentix.example.toml "$HOME\.config\agentix\config.toml"
```

In `config.toml`:

1. Enable one or more named backend tables: `[agent.codex]`, `[agent.pi]`, `[agent.omp]`, `[agent.claude]`. No agent `kind` field is needed.
2. Select one IM transport with `[channel].kind`: `telegram`, `feishu`, or `slack`.
3. Configure the matching `[channel.telegram]`, `[channel.feishu]`, or `[channel.slack]` table.
4. Leave the selected channel's owner list empty for first-time claiming, or add the owner IDs directly.

For Codex, keep the standalone binary path explicit:

```toml
[agent.codex]
command = "~/.codex/packages/standalone/current/codex"
proxy_endpoint = "unix://~/.codex/app-server-control/app-server-control.sock"
endpoint = "unix://~/.codex/app-server-control/app-server-control-upstream.sock"
```

Agentix owns `proxy_endpoint` and forwards each client to `endpoint` (the upstream). Both settings are optional; the defaults use the paths above, respecting `CODEX_HOME`. Agentix starts a missing local upstream with `codex app-server --listen ENDPOINT`. Its own requests and notifications connect directly to that upstream. A successfully started upstream survives Agentix shutdown; its socket is preserved. Agentix cleans only its own proxy socket on graceful exit. Existing upstreams are reused on restart. OS adoption of the orphaned process does not provide automatic crash restart.

Start Agentix first, then connect terminals with `codex --remote unix://`. For a custom proxy, pass its exact address to `--remote`. Existing clients that bypass the proxy are not registered; reconnect them through the proxy. If the old Codex daemon occupies the default proxy socket, stop it before migration or choose another proxy path. Agentix never removes an existing listener to take its place.

Unix and `ws://HOST:PORT` support separate connections for multiple clients. `proxy_endpoint = "stdio://"` accepts one newline-delimited JSON client on Agentix's stdin/stdout; its upstream must still be Unix or WS. Standard I/O is not a discoverable shared listener. Unix peer credentials provide the client PID. WS connections always leave PID unknown, including local connections. Stdio has no authenticated peer PID. Session registration still follows the connection in those cases.

Sessions are registered from successful client `thread/start`, `thread/resume`, and `thread/fork` responses and removed on unsubscribe or connection close. `/sessions` reads this registry rather than matching working directories or listing every loaded app-server thread. `agentix client call agentix/clients` exposes connection IDs, client names, PIDs, and session IDs for diagnostics. Restarting the proxy closes its clients; they must reconnect to register again. Saved IM bindings remain offline without blocking service startup until their original clients register again. Agentix then restores the attachments, using read-only access when another process owns the writer.

For a WS proxy listening beyond loopback, authentication is mandatory. Configure the inbound proxy separately from the upstream:

```toml
[agent.codex.proxy]
ws_auth = "capability-token"
ws_token_file = "/absolute/path/codex-proxy.token"
# Alternatively use ws_token_sha256 = "<64 hex digits>" instead of ws_token_file.
```

For signed bearer tokens, use `ws_auth = "signed-bearer-token"` with `ws_shared_secret_file` (at least 32 bytes after trimming), and optionally `ws_issuer`, `ws_audience`, and `ws_max_clock_skew_seconds` (default 30). Tokens must use HS256 with an integer `exp`; `nbf`, issuer, and audience are validated when present/configured. Token and secret file paths must be absolute after home expansion. Authentication files are loaded at startup; restart Agentix after rotation.

`agentix serve` accepts the equivalent seven `--ws-...` flags; explicitly supplied flags override corresponding TOML fields. These options authenticate the proxy's inbound WebSocket handshake, not the upstream connection. They are invalid on Unix/stdio listeners. Capability token file and digest are mutually exclusive. Invalid configuration terminates startup. Missing or invalid credentials receive HTTP 401 before upstream connection; browser Origin headers receive HTTP 403.

Native clients send the bearer through `codex --remote ws://HOST:PORT --remote-auth-token-env CODEX_PROXY_TOKEN`, with the token exported in that environment variable. For authenticated WS, configure the client environment/launcher accordingly; Agentix's rmux launcher does not distribute tokens. The proxy itself serves plaintext WS; use a trusted TLS terminator or SSH tunnel for encrypted remote access. Keep the shared upstream private; upstream bearer authentication is not configured by these flags.

The CLI receives a successful WebSocket upgrade only after the upstream confirms it. Upstream HTTP rejections are forwarded; unavailable or invalid upstream connections return HTTP 502, and an upstream handshake exceeding ten seconds returns HTTP 504. Authentication failures still return 401/403 before opening an upstream connection. Handshake response writes have a ten-second deadline; HTTP rejections close as soon as their declared Content-Length or chunked body completes, without waiting for upstream keep-alive closure. These deadlines do not apply to established WebSocket traffic. IPv6 addresses are supported for both the proxy and internal upstream connections using bracketed URLs such as ws://[::1]:4500; an unavailable IPv6 loopback upstream can be started locally. WS URLs default to port 80, including an explicit :80. Proxy and upstream must refer to different Unix sockets even when their paths use directory aliases or symbolic links; collisions are rejected at startup. WS upstreams also cannot resolve to the proxy’s own TCP listener, even with a different URL path or hostname.

The proxy does not send periodic Ping frames or disconnect clients for missing Pongs. It forwards Ping/Pong unchanged in both directions without responding locally. Neither endpoint gains proactive heartbeats from the proxy; see native Codex v0.154.0 [CLI](https://github.com/openai/codex/blob/rust-v0.154.0/codex-rs/app-server-client/src/remote.rs) and [app-server](https://github.com/openai/codex/blob/rust-v0.154.0/codex-rs/app-server-transport/src/transport/websocket.rs) direct WS behavior. Idle connections remain registered. Both Close frames are forwarded before the proxy shuts down and releases the two connections and clears their associations. EOF or a transport error also triggers cleanup; a network partition without such a signal has no guaranteed detection deadline. If message observation fails or exceeds its size limit, raw forwarding continues but session tracking in that direction stops until reconnection; cleanup then relies on transport termination. The proxy adds no closing timeout. The periodic registry reconciliation is not a network heartbeat. `stdio://` is a JSONL protocol adapter, so it has no frontend WS control frames; it still terminates upstream Ping/Pong locally. After upstream Close, its registration and upstream socket are released before queued stdout data drains. Pipe/terminal I/O is cancellable, so shutdown does not wait for an extra input byte.

Fill in the actual credentials for the selected channel in `config.toml`:

```toml
[channel.telegram]
token = "your-telegram-bot-token"

# Or, when channel.kind = "feishu":
[channel.feishu]
app_id = "your-feishu-app-id"
app_secret = "your-feishu-app-secret"
```

Credentials are read directly from this file, including when running as a Homebrew service. On macOS/Linux, restrict access with `chmod 600 ~/.config/agentix/config.toml`.

To configure the global outbound proxy, add a separate top-level table:

```toml
[network]
proxy = "http://127.0.0.1:7890"
```

Use your proxy's actual address and port. HTTP, HTTPS, SOCKS5, and SOCKS5h proxies are supported. The setting covers all Telegram requests and Slack HTTP/Socket Mode connections and works without shell proxy variables. The Feishu SDK does not use this setting and retains its existing network behavior. After changing it, restart a running Homebrew service with `brew services restart tenfyzhong/tap/agentix`.

See [Configuration and operations](development-and-operations.md) for backend details, Feishu permissions, logging, service management, and diagnostics.

### Start

Start Agentix:

```sh
agentix serve
```

With Codex, run `agentix doctor` from another terminal after startup, then connect the CLI with `codex --remote unix://`. The `proxy_endpoint` socket is reserved for Agentix: do not launch app-server on it. An occupied address causes an ERROR log and immediate nonzero exit. See [proxy setup and recovery](development-and-operations.md#codex).

On Windows, use `agentix.exe doctor` and `agentix.exe serve`. A Homebrew installation can run in the background instead:

```sh
brew services start tenfyzhong/tap/agentix
```

For a source build that is not on `PATH`, replace `agentix` with `target/release/agentix` in the commands above.

If the selected channel has no configured owner, keep `agentix serve` running, execute `agentix client claim` in another local terminal, and send the printed `/claim <code>` command to the bot in a private chat.

## Capabilities and session behavior


- Native Codex app-server integration plus Pi and OMP bridges inside the original host processes
- Telegram long polling, Feishu long connections, and Slack Socket Mode with interactive actions
- A duplex FIFO message center for IM traffic, with ordered retries at the outbound queue head
- A global HTTP/HTTPS/SOCKS5 proxy configured in TOML, including for Homebrew services
- Running-session discovery, attachment, history, prompts, queues, steering, stopping, approvals, and user-input round trips
- Codex controls for models, reasoning, Fast mode, plans, goals, reviews, diffs, forks, compaction, skills, and MCP servers
- Interactive rmux workspace browsing and safe Codex, Pi, and OMP session creation from IM
- Owner allowlists, one-time owner claiming, group mention requirements, event deduplication, and single-use actions
- Durable bindings, restart recovery, process-exit notifications, and automatic Codex reattachment
- Streamed in-place responses, background completion notifications, and reply context
- Standalone `taskix`: SQLite jobs with dependencies displayed as Mermaid graphs showing seven task statuses in a light palette shared with TaskNotes and clickable note links, concurrent task claims with lease release on supported host interruptions and session shutdown, task notes tagged `task` and `agent/task` with prerequisite and revision metadata, audit events, a compact clickable project Dashboard Base, and generated TaskNotes boards containing project metadata in Obsidian vaults
- Optional IM task controls and a shared Codex, Claude, Pi, and OMP plugin, with stable interfaces for future Agent Team orchestration

While `agentix serve` is running, Agentix checks running Codex sessions for completed turns every ten seconds using read-only history queries, including sessions that have never been attached or were detached from IM. Background monitoring does not resume sessions or acquire their writer locks. New completions include the completed turn's prompt and response, a Background label, and an Attach button. Feishu uses a purple header and a grey quote area; Telegram uses a ⚫ Background marker and blockquotes. Notifications go to authenticated IM conversations known to the service. Codex subagent sessions do not generate standalone completion notices; parent sessions and existing attached or draining turn cards continue updating normally. Send the bot `/help` once to register a conversation for these notifications; attaching a session is optional.

Before a Codex session's first user message, background history reads may report that the thread is not materialized yet. Agentix logs this expected condition at debug level and keeps polling; other background read errors remain warnings.

Attaching a session restores its latest turn with a Stop button when that turn is running and writable. If another Codex process owns the session's writer, Agentix connects read-only, restores the latest saved content, and checks for updates every ten seconds. Proxy-backed attachments also follow client disconnection and reconnection; reappearance preserves read-only access without acquiring the writer. Detaching stops their lifecycle monitoring. The menu keeps history and navigation commands; sending prompts and changing the session require the original Codex process. Session lists infer external-session activity from the latest saved turn when live status is unavailable. Other attachment failures show their reason and a fresh Retry attach action.

Only the current attached session's writable active turn message has Stop; switching sessions, moving the attachment to another conversation, detaching, or finishing the turn removes it from the previous message. Copies shown by `/history` never include Stop.

Starting `agentix serve` restores saved conversations and sends their online status without posting command menu cards to Feishu or Slack. Telegram's native command menu is still synchronized. Send `/help` to view the available commands.

Startup recovery, automatic reattachment, and shutdown notifications only use channels enabled in the current configuration. Saved bindings and turn messages for other channels are retained for when those channels are enabled again. Each IM adapter and its clones share a duplex FIFO message center. Normalized incoming messages use an independent inbound queue; sends, edits, menus, owner-claim replies, callback acknowledgements, and Feishu reply lookups use the outbound queue. A rate-limited request stays at the head until it succeeds, fails permanently, or is cancelled, so later requests cannot overtake its retries. Telegram honors `retry_after` and spaces requests globally and per chat. Feishu HTTP 429 responses use exponential backoff from one second up to 60 seconds because its SDK does not expose the server retry delay. Cancelling a request removes that operation while preserving the channel cooldown. Telegram streams and working-duration updates refresh at most once every five seconds. Final turn updates bypass that refresh interval while still respecting Telegram cooldowns. Telegram rate-limit logs include the API method and chat ID.

To disable completion notices for unattached sessions, add this to `config.toml` and run `agentix reload`:

```toml
[notifications]
background_turns = false
```

The default is `true`. Disabling notifications also stops automatic background turn polling and full-content reads. When no sessions need exit/resume monitoring, automatic session discovery stops too. Existing attached or draining turn cards still complete in place; attached-session exit/resume monitoring remains active. Both completion deduplication caches keep only the latest completed turn per session, with recipient tracking for that turn, so records do not accumulate for every completed turn.

## IM task boards

Task document cleanup preserves unrelated files at destinations where projection failed. Authored Plan frontmatter supports LF and CRLF delimiters and quoted YAML keys. Pi/OMP lease injection accepts full Task IDs and unambiguous Task prefixes.

Add the following to `~/.config/agentix/config.toml`, then run `agentix reload` to browse work in Telegram, Feishu, or Slack:

```toml
[task_board]
enable = true
config = "~/.config/taskix/config.toml"
```

`task_board.enable` defaults to `false`, including when the section contains only a `config` path. When disabled, Agentix does not load the taskix configuration or start the task board, and IM menus and help omit task-board commands. Set `enable = true` to enable the integration. The referenced taskix configuration must then already exist. Agentix opens the database specified there and creates it if missing; use the same configuration as your taskix writers to see their existing work. Run `agentix reload` after changing this section. When disabled, typed board commands report `Task board is not configured.`

| Command | View |
| --- | --- |
| `/dashboard` | Project dashboard; click a project to open its task board |
| `/board` | The attached session's task board; click a task for its Markdown details |
| `/jobs` | All Jobs associated with the attached session; click a Job for its Markdown details |

`/dashboard` is registered in Telegram’s default menu at startup when `task_board.enable = true`. Top-level commands appear in the order `/sessions`, `/dashboard`, `/cancel`, `/rmux`, `/help`; contextual commands follow in alphabetical order. `/board` and `/jobs` are contextual secondary commands added after attach and removed on detach. Both follow the current attachment, including Jobs containing tasks whose lease or last recorded session matches it. Sibling tasks show overall Job progress; blocked and completed work remains visible after lease release. Archived projects and Jobs are excluded from lists.

Job details display the authored Goal and Notes, plus buttons for associated tasks. Task details display the authored Task note body, status and planning/execution phase, with a **Job** button to open the parent Job. Existing Task action buttons remain available where ownership permits. Lists and long Markdown details, including Task reasons, have **Previous**/**Next** buttons; code fences remain balanced across detail pages. Detail headers shorten long titles to 60 characters; their full titles remain in the paginated body. Project boards include a **Dashboard** button, and Job details link to their **Project board**.

Browsing reads current task data without changing task state or Plan hashes. Navigation buttons are scoped to the conversation, owner, and current attachment; reopen the command after switching sessions. The earlier `/projects` and `/sessionboard` commands are replaced by `/dashboard` and `/board`. Legacy `/tasks [job-or-project]` and `/task <id>` remain direct shortcuts. See the [task board guide](task-board.md#agentix-integration) for setup and task actions.

## Documentation

- [Usage guide](usage.md)
- [Configuration and operations](development-and-operations.md)
- [Contributing](../CONTRIBUTING.md)
- [Product design](product-design.md)
- [Internal architecture and message flow diagrams](architecture.md): service components, duplex FIFO queues, and rate-limit retry ordering
- [Task board, standalone CLI, and agent plugin](task-board.md)
- [Task workflow responsibilities and lifecycle](task-workflow-mechanisms.md)
- [Integration coverage and live acceptance boundaries](integration-coverage.md)

## Live Pi and OMP sessions

Install the [Agentix bridge](../plugins/agentix-bridge/README.md) in the original terminal before attaching. Pi/OMP extensions connect to the Unix socket owned by `agentix serve` and retry in the background if it is unavailable. A service can enable `[agent.codex]`, `[agent.pi]`, `[agent.omp]`, and `[agent.claude]` together. Sessions use backend-qualified IDs, while native task leases keep their original IDs. Commands reflect each host's available capabilities; Pi/OMP busy-session messages enter FIFO and `/steer` explicitly steers. Claude Code connects through `[agent.claude]` and the [plugin with rmux input](claude-code.md), with prompt, history, and status capabilities.
