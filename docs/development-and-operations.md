# Configuration and Operations

## Configuration

Copy `config/agentix.example.toml` to `$HOME/.config/agentix/config.toml` and select exactly one agent backend. This is the default path when `--config` is omitted. The required `[channel].kind` field selects exactly one active IM transport: `telegram` or `feishu`. Its matching `[channel.telegram]` or `[channel.feishu]` table must exist. Either channel may start with an empty owner list so its one-time claim flow can initialize the allowlist. The inactive nested channel table may remain in the file, but Agentix does not validate its owner list, read its credentials, or start its adapter.

Store the actual credentials in TOML: `channel.telegram.token` for Telegram, or `channel.feishu.app_id` and `channel.feishu.app_secret` for Feishu. The selected channel's credentials must be present and nonblank. Agentix reads them directly from the file, so Homebrew services need no credential environment variables. Restrict the file to your user (`chmod 600 ~/.config/agentix/config.toml` on macOS/Linux).

Every filesystem path accepts `~` or `~/...` and expands it to the current user's home directory. This includes `storage.path`, agent commands, Pi/OMP session directories, and the path portion of Agentix or Codex `unix://` endpoints. Named-user forms such as `~someone` and environment variables such as `$HOME` are not expanded.

Set `agent.rmux_directory` to choose the workspace used when `/rmux` creates a session, window, or pane; it defaults to the current user's home directory. The former `agent.multiplexer_directory` key remains available as a compatibility alias.

### Global outbound proxy

Configure the global outbound proxy in a top-level table:

```toml
[network]
proxy = "http://127.0.0.1:7890"
```

`network.proxy` accepts `http://`, `https://`, `socks5://`, and `socks5h://` URLs. Use `socks5h://127.0.0.1:1080` when the proxy should resolve destination hostnames. Authentication can be supplied as URL-encoded user information, for example `http://username:password@127.0.0.1:7890`. Proxy URLs must have a host and may have a port; paths, query strings, fragments, and blank values are rejected during configuration validation.

The configured proxy takes precedence over environment proxy settings, including bypass rules, for clients using this setting. It covers all Telegram requests, including polling, menus, messages, edits, and callback acknowledgements. Proxy failures return errors; these requests do not fall back to a direct connection. Omit `network.proxy` to retain the client's existing routing behavior.

The Feishu SDK does not use `network.proxy`. Its token requests, OpenAPI calls, WebSocket bootstrap, and WebSocket connections retain their existing network behavior.

Local control connections, Codex Unix sockets, and Pi/Oh My Pi RPC pipes remain local. Coding agents already running on your computer retain their own provider-network settings.

Homebrew services read the same configuration file and need no shell proxy variables. After editing the file, restart the service:

```sh
brew services restart tenfyzhong/tap/agentix
```

### Local control endpoint

`agentix serve` exposes a local newline-delimited JSON control endpoint used by every `agentix client` subcommand. The platform defaults are:

- macOS/Linux: `unix://~/.local/share/agentix/control.sock`
- Windows: `tcp://127.0.0.1:32198`

Override the endpoint with `[server].endpoint`. Explicit TCP endpoints are accepted on every platform but must use a numeric loopback address; remote binds are rejected. On Unix, Agentix creates parent directories, sets the socket mode to `0600`, rejects a live duplicate server, removes a stale socket, and removes its socket on graceful shutdown.

The control protocol handles one request and one response per connection. `client sessions` goes through the running adapter, `client call` is passed through the server's existing Codex app-server connection, and `client claim` creates claim state inside the running server. Consequently, all client commands require `agentix serve` to be running and use the same backend state as the IM channel.

### Codex

The Codex adapter requires Codex CLI 0.153.0 or newer from OpenAI's official standalone installer:

```sh
curl -fsSL https://chatgpt.com/codex/install.sh | sh
```

The Homebrew package is not compatible with this integration because it does not install the managed standalone app-server layout. Configure `agent.command = "~/.codex/packages/standalone/current/codex"`; do not point Agentix at a Homebrew `codex` executable.

Agentix starts the managed daemon automatically when the default control socket is missing or refusing connections, waits for it to become ready, and then completes the app-server handshake. Verify the integration with:

```sh
~/.codex/packages/standalone/current/codex --version
~/.codex/packages/standalone/current/codex app-server daemon version
agentix doctor
```

`endpoint = "unix://"` resolves to the current Codex home control socket. `command = "codex"` selects the executable used for `codex app-server daemon start`; use an absolute or `~/...` path when the service environment has a restricted `PATH`. A custom endpoint must resolve to an absolute Unix socket path, for example `unix://~/.codex/custom.sock`, and is never auto-started. TCP WebSocket endpoints are intentionally rejected by this release.

For the managed `unix://` endpoint, Agentix uses `ps` and `lsof` to correlate interactive Codex TUI processes with standalone writer locks and daemon-backed threads. Both commands must be available on `PATH`. Inactive sessions persisted on disk and orphaned daemon threads are not listed. Custom socket endpoints fall back to the app-server's `thread/loaded/list` view.

### Pi

Configure the executable and session root, normally `~/.pi/agent/sessions`. The executable must support `--mode rpc` and `--session <path|id>`.

### Oh My Pi

Configure `kind = "oh-my-pi"`, the `omp` executable, and its JSONL session root. Agentix resumes each file with `--resume` and uses protocol-v1 JSONL framing for compatibility.

## Optional task coordination

Initialize taskcli separately, then set `[task_board].config` to its configuration path to enable IM task browsing and controls. The task database and document tree are independent of Agentix binding storage. Install the host plugin separately for CLI-session lease hooks; running Agentix is not required for standalone taskcli use. See the [task board guide](task-board.md) for configuration, document migration, backup, and session cleanup.

## Owner claim setup

For first-time Telegram or Feishu setup, omit the selected channel's owner list or set it to `[]`:

- Telegram: `channel.telegram.owner_user_ids`
- Feishu: `channel.feishu.owner_open_ids`

Start `agentix serve`, then generate a temporary claim code from another local terminal:

```sh
agentix client claim
agentix client claim --ttl-minutes 30
```

The command prints a ready-to-send line such as:

```text
/claim 12AB34CD56EF
```

The default lifetime is 10 minutes; `--ttl-minutes` accepts 1 through 1440. The plaintext is written only to the command's stdout and is never logged. The running server keeps one pending code and its expiry only in memory. Generating a new code replaces the previous code, and restarting `serve` invalidates it. Neither the code, a hash, nor its expiry is written to TOML or SQLite.

Send the command to the selected bot in a private chat. The adapter compares it with the shared in-memory registry and obtains the numeric Telegram user ID or Feishu `open_id` from the official message event. A successful match atomically adds that ID to the selected channel's owner list in the config file, consumes the code, and enables the owner immediately. There is no `--write` option or restart step. The claim message is consumed by the channel and is never forwarded to the coding agent. Generation exists only through the local Agentix control client; it is not available from remote IM. Claims are rejected after expiry, after the first success, when an owner is already configured, and in group messages.

## Feishu app setup

Create a bot application and configure event delivery through a long connection. Enable message receive events and card action callbacks. Grant the minimum bot scopes needed to read messages and send or edit interactive messages. Reply context requires either `im:message` or `im:message:readonly`; quoting group messages additionally requires `im:message.group_msg`. Add the bot to each intended group.

Agentix requires a mention in groups. The Feishu SDK acknowledges card actions within the callback window before the core executes the action.

## Telegram bot setup

Create a bot token and set `channel.telegram.token` to its value. Add numeric owner user IDs directly or initialize the first owner through the claim flow above. In group chats, privacy mode and bot permissions must still allow mentioned messages to reach the bot. Agentix ignores unmentioned group text.

At channel startup, Agentix registers Telegram's primary commands in the order `/sessions`, `/dashboard`, `/cancel`, `/rmux`, `/help`, omitting `/dashboard` when task boards are not configured. It selects the commands menu button for private chats. After attachment, contextual commands follow in alphabetical order, including `/board` and `/jobs` when task boards are enabled, and clickable `/model` and `/reasoning` selectors for Codex; `/thinking` is not exposed. Menu registration is refreshed on every restart, so BotFather command configuration is not required. `/attach` is intentionally omitted from the menu because it requires a session ID; use the title buttons from `/sessions` instead.

## Service operation

Run in the foreground:

```sh
RUST_LOG=agentix=info agentix serve
```

Without `RUST_LOG`, `[logging].level` supplies the tracing filter. `[logging.file]` can enable a second, ANSI-free destination with a Home-relative path, `never`, `minutely`, `hourly`, or `daily` rotation, and a positive `max_files` retention count. Agentix creates the parent directory before initializing the appender. Both stderr and file logs use the computer's local RFC 3339 time.

Use the operating system's user service manager for production. Run the process as the same user that owns the coding-agent session files and Codex socket. Restrict the configuration file containing credentials to that user.

Graceful shutdown cancels channel listeners, stops inbound/event loops, checkpoints SQLite WAL state, removes live turn controls, restores detached IM menus, and sends an offline notification. Durable bindings are retained without stopping the coding-agent session. Channel adapters share one five-second shutdown deadline, so multiple adapters do not multiply the wait. Active turn text, status, owner context, and IM message references are checkpointed so a restart refreshes the existing Stop action and later completion edits the original message instead of creating a duplicate.

Agentix restores durable bindings and turn state before starting its control and IM tasks. Menu updates, online notices, and restored turn displays run in the background, so slow IM requests do not delay `Agentix is running`. Pending startup updates are cancelled before shutdown notices, and updates for bindings that changed are skipped. The running log does not mean the IM connection is ready; Telegram logs separately when initialization finishes and polling starts.

If the agent rejects a saved session because it is no longer attachable, Agentix removes that stale binding, keeps the IM detached, and reports the result.

For the managed Codex socket, Agentix polls the local interactive process set every ten seconds and confirms an attached-session exit after two consecutive missing snapshots. It then notifies the bound IM conversation, removes live controls, and suspends the binding while continuing to watch that session ID. Running `codex resume` for the same session restores the app-server subscription and IM binding automatically. A manual detach or attaching another session cancels that watch. App-server disconnects, `thread/closed`, and `notLoaded` do not suspend the durable binding. Custom Codex sockets do not automatically detect process exit or resume because their process tree is not locally discoverable.

## Diagnostics

Startup logs include `phase` and `elapsed_ms` for the agent connection, channel/task setup, state storage, binding restoration, and background IM presentation. The `elapsed_ms` on `Agentix is running` measures service setup from opening state storage; the preceding setup phases are logged separately.

`agentix doctor` checks:

- TOML structure and selected-channel owner configuration
- required credentials in the configuration file without printing values
- global proxy URL validity (proxy connectivity is exercised when the service connects)
- state directory existence
- Codex managed-daemon startup plus initialize/list handshake, or Pi/OMP executable and session discovery

Useful operational checks:

```sh
agentix doctor
agentix client sessions
agentix client call thread/loaded/list --params '{"limit":10}' | jq
agentix client call thread/queue/list --params '{"threadId":"019...","limit":100}' | jq
agentix client claim --ttl-minutes 10
codex app-server daemon version
RUST_LOG=agentix=debug agentix serve
```

`agentix client sessions` works with every configured backend and emits a normalized JSON page. `agentix client call` is Codex-specific and asks `serve` to send the supplied JSON parameters over its existing app-server connection. Diagnostic JSON is written to stdout and tracing output to stderr. If `serve` is unavailable, the client reports that it could not connect to the configured Agentix control endpoint.

Tracing timestamps use RFC 3339 in the computer's local time zone and include its UTC offset. For example, a machine configured for Asia/Shanghai emits `2026-09-04T11:42:22.975758+08:00` rather than the equivalent UTC timestamp ending in `Z`.

## Compatibility and limits

- Rust 1.95 or newer is required by the pinned Feishu SDK.
- The Codex backend requires the official standalone Codex CLI 0.153.0 or newer. Homebrew installations do not include the managed app-server layout required by Agentix.
- One service process selects one agent backend and one IM channel. Run separate config/state instances to expose different backends or channels at the same time.
- Telegram converts standard Markdown in quoted agent replies to MarkdownV2 for sends and streamed edits, preserving paragraph and list boundaries inside the quote. Automatic link previews are disabled to keep structured bot responses free of unrelated webpage cards. Rendered output is conservatively bounded to 4,096 UTF-8 bytes.
- Feishu card body output is bounded before transport.
- Pi/OMP session listing scans JSONL files recursively; very large stores should be indexed in a later release.
- Codex's persistent queue API is experimental. External queue entries execute automatically, but Codex CLI 0.153.0 keeps Tab-submitted follow-ups in a private, in-process TUI queue and ignores `thread/queue/changed`. That local queue and the app-server queue used by Agentix do not synchronize or deduplicate through the official protocol. The TUI may not show an Agentix queue entry until its turn starts, and Agentix cannot list a Tab-queued TUI entry; `/queue` is authoritative only for the app-server queue. If both queues contain input when a turn ends, each owner may try to submit its next item, producing back-to-back turns with no shared ordering guarantee. Do not use both queues concurrently for the same session.
- Claude Code IM support is deferred until a stable, authenticated control transport and approval protocol are selected. Claude Code can already use the standalone task plugin.

For the development workflow, test architecture, CI, and release process, see [Contributing to Agentix](../CONTRIBUTING.md).
