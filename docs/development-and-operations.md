# Configuration and Operations

## Configuration

Copy `config/agentix.example.toml` to `$HOME/.config/agentix/config.toml` and enable one or more named backend tables: `[agent.codex]`, `[agent.pi]`, `[agent.omp]`, `[agent.claude]`. The table name selects the backend, so omit its `kind` field. This is the default path when `--config` is omitted. The required `[channel].kind` field selects exactly one active IM transport: `telegram`, `feishu`, or `slack`. Its matching `[channel.telegram]`, `[channel.feishu]`, or `[channel.slack]` table must exist. Any channel may start with an empty owner list so its one-time claim flow can initialize the allowlist. Inactive nested channel tables may remain in the file, but Agentix does not validate their owner lists, read their credentials, or start their adapters.

Store the actual credentials in TOML: `channel.telegram.token` for Telegram, or `channel.feishu.app_id` and `channel.feishu.app_secret` for Feishu; `channel.slack.bot_token` and `channel.slack.app_token` for Slack. The selected channel's credentials must be present and nonblank. Agentix reads them directly from the file, so Homebrew services need no credential environment variables. Restrict the file to your user (`chmod 600 ~/.config/agentix/config.toml` on macOS/Linux).

Filesystem paths other than the absolute `slack_cli_path` override accept `~` or `~/...` and expands it to the current user's home directory. This includes `storage.path`, agent commands, Pi/OMP session directories, and the path portion of Agentix or Codex `unix://` endpoints. Named-user forms such as `~someone` and environment variables such as `$HOME` are not expanded.

Set `multiplexer.kind` to `auto` (default), `rmux`, or `tmux`. At service startup, `auto` probes the native rmux protocol first, then the tmux command interface. Explicit `rmux` requires a successful native probe; explicit `tmux` accepts a successful tmux-compatible response, including rmux. Probes are read-only and never start a daemon. If no applicable probe succeeds, terminal management is disabled and neither terminal command is registered. Start the desired server before Agentix; restart Agentix to detect a newly available server. `multiplexer.working_dir` is shared by every agent and defaults to `"~"`, expanded to the current user’s home directory. The selected menu is `/rmux` or `/tmux`. Both settings require a service restart. Per-agent `rmux_directory` and `multiplexer_directory` have been removed and are rejected.

The `/rmux` and `/tmux` menus create an empty shell pane for `+ Session`, `+ Window`, and either split action, then show the configured agents (Codex, Claude Code, Pi, or Oh My Pi). Select an agent to start it in that exact pane and attach the IM conversation. Until a selection is made, the pane remains an ordinary shell. Existing idle panes expose `Run agent` with the same selection step, even when the conversation is already attached to a different agent.

On macOS and Linux, agents launched from the `/rmux` or `/tmux` menu return to an interactive `$SHELL` (or `/bin/sh` when unset) after exiting, including unsuccessful exits. The pane remains usable in the configured working directory. Exiting that shell with Ctrl-D or `exit` closes the pane normally. This applies to new sessions, windows, splits, and reused panes; already dead panes are not revived by this change.

### Slack CLI startup synchronization

Set `channel.slack.app_id` to enable startup slash-command synchronization through a logged-in Slack CLI. Run `slack login` as the service user. The global `slack_cli_path` option, placed before all TOML tables, optionally specifies an absolute executable path when `slack` is not on PATH. CLI authorization and refresh are managed by Slack CLI; Agentix does not store management tokens. Synchronization failures log a warning and allow Socket Mode to start with existing commands. See [Slack initialization and integration](slack-initialization.md).

### Global outbound proxy

Configure the global outbound proxy in a top-level table:

```toml
[network]
proxy = "http://127.0.0.1:7890"
```

`network.proxy` accepts `http://`, `https://`, `socks5://`, and `socks5h://` URLs. Use `socks5h://127.0.0.1:1080` when the proxy should resolve destination hostnames. Authentication can be supplied as URL-encoded user information, for example `http://username:password@127.0.0.1:7890`. Proxy URLs must have a host and may have a port; paths, query strings, fragments, and blank values are rejected during configuration validation.

The configured proxy takes precedence over environment proxy settings, including bypass rules, for clients using this setting. It covers all Telegram requests and Slack API/WebSocket traffic. Telegram coverage includes polling, menus, messages, edits, and callback acknowledgements. Proxy failures return errors; these requests do not fall back to a direct connection. Omit `network.proxy` to retain the client's existing routing behavior.

The Feishu SDK does not use `network.proxy`. Its token requests, OpenAPI calls, WebSocket bootstrap, and WebSocket connections retain their existing network behavior.

Local control connections, Codex Unix sockets, and Pi/Oh My Pi extension sockets remain local. Coding agents already running on your computer retain their own provider-network settings.

Homebrew services read the same configuration file and need no shell proxy variables. After editing the file, restart the service:

```sh
brew services restart tenfyzhong/tap/agentix
```

### Local control endpoint

`agentix serve` exposes a local newline-delimited JSON control endpoint used by every `agentix client` subcommand. The platform defaults are:

- macOS/Linux: `unix://~/.local/share/agentix/control.sock`
- Windows: `tcp://127.0.0.1:32198`

Override the endpoint with `[server].endpoint`. Explicit TCP endpoints are accepted on every platform but must use a numeric loopback address; remote binds are rejected. On Unix, Agentix creates parent directories, sets the socket mode to `0600`, rejects a live duplicate server, removes a stale socket, and removes its socket on graceful shutdown.

Ordinary control clients exchange one request and one response per connection. Native extensions register on the same Unix listener and retain a bidirectional connection. `client send`, `stop`, `history`, and `command` use shared session operations; `client sessions` goes through the running adapter, `client call` is passed through the server's existing Codex app-server connection, and `client claim` creates claim state inside the running server. Consequently, all client commands require `agentix serve` to be running and use the same backend state as the IM channel.

### Reloading configuration

After editing the configuration used by the running service, run:

```sh
agentix reload
# For a service started with a custom configuration:
agentix --config /path/to/config.toml reload
# Connect directly if the local file is invalid or its endpoint was edited:
agentix reload --endpoint unix:///path/to/control.sock
agentix reload --endpoint tcp://127.0.0.1:46783
```

The command sends a `{"method":"reload"}` request to the local control endpoint. The server reads its own original configuration path; `--config` on the client only selects the endpoint. Explicit `--endpoint` skips loading the client's configuration. A successful command prints JSON containing `reloaded: true` and the server's configuration path. Invalid configuration, setup failures, and unsupported changes return an error and a nonzero exit status.

Reload supports the selected IM channel and credentials, owner lists, Slack command names and CLI path, network proxy, output settings, background-turn notifications, task-board settings (including rereading the referenced taskix configuration), and Pi/OMP/Claude backend additions, changes, and removals. Existing unchanged agent connections and the native bridge listener are retained. CLI proxy authentication options supplied to `serve` keep their precedence after every reload.

Changes to `server.endpoint`, `storage.path`, or any logging setting require restarting the service. Changing or removing an already configured Codex backend also requires a restart because its proxy owns a live listener. These changes reject the entire reload; they are never silently ignored. Adding Codex to a service that does not yet configure it is supported.

Preparation and replacement-credential validation run while the current service continues processing requests, each with a 30-second deadline. Overlapping reload requests receive a busy error. A failed preparation or validation leaves the current settings and connections intact.

Owner lists, output, notifications, and task-board settings switch in place. The control listener, native bridge hub, inbound queue, dispatch queues, and in-flight workers stay alive. Unchanged IM connections and backend adapters are reused, including their event subscriptions. Live binding epochs, pending interactions, and turn buffers are shared across configuration snapshots; reload does not restore bindings from disk or send online/offline notices. Already running work finishes with its original snapshot; subsequent work uses the new settings.

When IM credentials, proxy settings, or Slack command configuration change, Agentix validates the replacement before stopping only the affected receiver. It then starts the replacement on the same inbound queue. This is a best-effort handoff: protocols such as Telegram polling cannot run two consumers for the same bot, and establishing the new connection can still cause a brief receive delay. A successful reload means the configuration is installed, not that an external websocket or polling connection is already ready. Subsequent transport failures follow the adapter's normal behavior and are logged. If the receiver has exited, a subsequent reload retries it even when its connection settings are unchanged. Changing a bot's identity requires restart so its old bindings are reconciled safely. Backend changes can likewise require host reconnection and event-state recovery; finish active work before changing or removing its backend.

Reload regression tests cover receiver handoff under concurrent traffic, restart of a receiver that failed after credential validation, output and notification settings, task-board enable/disable and pending input, and recovery of an existing backend turn when the registry changes. The Telegram transport test uses the real adapter against a local mock HTTP API. Output visibility changes apply to subsequent items; already rendered process items remain in the turn history. Registry replacement relies on backend history to recover events missed during subscription handoff, so this does not guarantee delivery of transient events absent from history.

These deterministic tests do not certify live Telegram/Feishu/Slack delivery across a network outage. Live acceptance requires dedicated test bots and conversations: send uniquely numbered messages before, during, and after credential/proxy changes, compare accepted IDs and reply order, and exercise server redelivery and connection rejection. No live messages are sent by the normal test suite.


Task-board commands and help use the new settings immediately. Existing external command menus refresh on the next attachment or channel startup; the global Telegram menu and Slack manifest registration refresh at channel startup.

### Codex

The Codex adapter requires Codex CLI 0.153.0 or newer from OpenAI's official standalone installer:

```sh
curl -fsSL https://chatgpt.com/codex/install.sh | sh
```

The Homebrew package is not compatible with this integration because it does not install the managed standalone app-server layout. Configure `agent.codex.command = "~/.codex/packages/standalone/current/codex"`; do not point Agentix at a Homebrew `codex` executable.

`agentix serve` first binds `[agent.codex].proxy_endpoint`, then connects to the shared upstream configured by `endpoint`. The defaults are `unix://~/.codex/app-server-control/app-server-control.sock` and `unix://~/.codex/app-server-control/app-server-control-upstream.sock`, respectively; omitted values honor `CODEX_HOME`.

**The proxy endpoint belongs exclusively to Agentix. Do not start `codex app-server` on this address.** An existing listener, stale Unix socket, ordinary file, or occupied WS port makes `serve` log an ERROR and exit nonzero without retrying or deleting the existing path. If migrating from a daemon on the default address, stop it after active work finishes or select a different proxy endpoint. After an ungraceful Agentix exit, remove a residual proxy socket only after confirming no process is listening there. Agentix removes only its own unchanged socket on graceful shutdown.

When a local upstream is missing or refuses connections, Agentix runs the configured `command` with `app-server --listen ENDPOINT` and waits for readiness. Custom Unix and loopback WS upstreams support automatic startup too; remote WS upstreams must already be running. The ready upstream survives Agentix exit, and its socket is preserved for reuse. Adoption by init does not provide automatic crash restart.

Start `agentix serve`, then use another terminal:

```sh
agentix doctor
codex --remote unix://
agentix client call agentix/clients
```

With a custom proxy endpoint, pass that address to `codex --remote`. Unix and WS proxy listeners each forward clients through separate upstream connections. Each direction progresses independently under backpressure. Accept failures log an error and retry with 25 ms–1 s exponential backoff while existing clients remain connected. A CLI upgrade succeeds only after the upstream upgrade; upstream HTTP errors are forwarded, while connection/protocol failures return 502 and handshake timeouts return 504. `stdio://` is a newline-delimited JSON frontend for one client; the shared upstream must use Unix or WS. Its bounded output queue allows stdin EOF to cancel blocked stdout writes. Upstream Close releases the registration and network socket before output drains. Cancellable Unix standard I/O prevents runtime shutdown from waiting for input; descriptor flags are restored on drop. HTTP response writes are bounded to ten seconds, and rejection forwarding ends at the declared body boundary. Both listener and upstream support bracketed IPv6 URLs. Unix connections expose the kernel peer PID. WS and stdio leave PID unknown; session ownership is keyed by connection. Non-loopback WS requires proxy authentication; see the [seven authentication options and client setup](guide.md). Ping/Pong pass through unchanged without local replies. Both Close frames are forwarded before socket shutdown and registration cleanup. Observation errors disable only message inspection in that direction; raw forwarding continues until transport termination. No proactive heartbeat or Pong timeout is imposed; transport closure/errors clear registrations, while silent network partitions have no fixed detection deadline.

The live session registry follows successful `thread/start`, `thread/resume`, and `thread/fork` responses, successful unsubscribe, and connection closure. `/sessions` reads this registry and fetches thread metadata; it does not match working directories or treat `thread/loaded/list` as evidence of a connected CLI. Streamed notifications are inspected without building their complete JSON object; tracked responses are parsed fully. WebSocket payloads are forwarded unchanged. Clients that bypass the proxy are not listed, and clients must reconnect through it after an Agentix restart.

### Login shell environment

Before starting the shared Codex upstream, Agentix looks up the effective user's login shell in the system account database and runs it with `-lc` to read all exported environment variables. The complete snapshot becomes the Codex startup command's environment, including PATH, newly exported variables, overridden values, and removals made with `unset`. Agentix's own environment is unchanged. The Homebrew formula can continue to run `agentix serve` directly, without a fish dependency or a fixed user-specific PATH.

Shell configuration must export variables for noninteractive login shells (for example, `set -gx` in fish). Shell-local variables are not inherited. For fish, keep these settings outside `if status is-interactive` blocks. Variables set temporarily in a terminal are not recovered. To select a different shell, set `AGENTIX_LOGIN_SHELL` to its absolute executable path in Agentix's service environment. This override must support `-lc` and the environment snapshot command (for example fish, bash, or zsh).

The lookup has a three-second timeout. If the account lookup or shell fails, or the environment output is malformed, Agentix logs a warning and starts Codex with its original inherited environment. No partial snapshot is applied. NUL-delimited entries preserve empty values, spaces, newlines, equals signs, and non-UTF-8 bytes; shell startup output is separated from the snapshot. Environment values are not written to logs.

An already running upstream is reused and retains its existing environment. To apply changed environment settings, stop Agentix and the upstream after active work finishes, then start Agentix again so it creates the upstream with the login shell environment.

### Pi

Configure the executable and session root, normally `~/.pi/agent/sessions`, and load the [Agentix bridge extension](../plugins/agentix-bridge/README.md) in Pi. The extension connects to the Unix socket owned by `agentix serve` and controls the original running session through native APIs.

### Oh My Pi

Configure `[agent.omp]`, the `omp` executable, and its session root. Load the OMP bridge extension; it connects to the same Agentix Unix listener as Pi. Attaching does not spawn or resume another process. Extensions reconnect in the background after service restart.

### Claude Code

Configure `[agent.claude]` and install `agentix-bridge@agentix`. For default IM input, install rmux or tmux on PATH and run Claude inside it. The optional Channel mode requires its environment selection and startup flag. All modes use the existing control socket; see [Claude setup and delivery modes](claude-code.md).

### Named backend configuration

```toml
[agent.codex]
command = "~/.codex/packages/standalone/current/codex"
proxy_endpoint = "unix://"
endpoint = "unix://~/.codex/app-server-control/app-server-control-upstream.sock"

[agent.omp]
command = "omp"
session_dir = "~/.omp/agent/sessions"

[agent.pi]
command = "pi"
session_dir = "~/.pi/agent/sessions"

[agent.claude]
command = "claude"
session_dir = "~/.claude/projects"
```

Omit a backend table to disable it. Existing backend-specific fields retain their meanings and defaults; Pi/OMP still require `session_dir`. Unknown names, unknown backend fields, and a `kind` field inside a named table are rejected. Legacy `[agent]` with `kind` and `[[agents]]` arrays remain accepted for migration, but cannot be mixed with named tables. Run `agentix reload` after changing supported runtime configuration; see [reload limitations](#reloading-configuration). An old unqualified binding database must first be opened with its original single backend so migration does not guess ownership.

## Optional task coordination

Initialize taskix separately, then set `[task_board].config` to its configuration path and `task_board.enable = true` to enable IM task browsing and controls. The enable switch defaults to `false`; while disabled, Agentix does not load the taskix configuration, start the task board, or add its commands to IM menus and help. The task database and document tree are independent of Agentix binding storage. Install the host plugin separately for CLI-session lease hooks; running Agentix is not required for standalone taskix use. See the [task board guide](task-board.md) for configuration, document migration, backup, and session cleanup.

## Owner claim setup

For first-time Telegram, Feishu, or Slack setup, omit the selected channel's owner list or set it to `[]`:

- Telegram: `channel.telegram.owner_user_ids`
- Feishu: `channel.feishu.owner_open_ids`
- Slack: `channel.slack.owner_user_ids` (see [Slack setup](slack.md))

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

Send the command to the selected bot in a private chat. Slack initially registers only `/claim`; successful enrollment replaces it with the normal command menu. The adapter compares it with the shared in-memory registry and obtains the numeric Telegram user ID, Feishu `open_id`, or Slack member ID from the official message event. A successful match atomically adds that ID to the selected channel's owner list in the config file, consumes the code, and enables the owner immediately. There is no `--write` option or restart step. The claim message is consumed by the channel and is never forwarded to the coding agent. Generation exists only through the local Agentix control client; it is not available from remote IM. Claims are rejected after expiry, after the first success, when an owner is already configured, and in group messages.

## Feishu app setup

Create a bot application and configure event delivery through a long connection. Enable message receive events and card action callbacks. Grant the minimum bot scopes needed to read messages and send or edit interactive messages. Reply context requires either `im:message` or `im:message:readonly`; quoting group messages additionally requires `im:message.group_msg`. Add the bot to each intended group.

Agentix requires a mention in groups. The Feishu SDK acknowledges card actions within the callback window before the core executes the action.

## Telegram bot setup

Create a bot token and set `channel.telegram.token` to its value. Add numeric owner user IDs directly or initialize the first owner through the claim flow above. In group chats, privacy mode and bot permissions must still allow mentioned messages to reach the bot. Agentix ignores unmentioned group text.

At channel startup, Agentix registers Telegram's primary commands in the order `/sessions`, `/dashboard`, `/cancel`, `/rmux` (or `/tmux` according to configuration), `/help`, omitting `/dashboard` when `task_board.enable` is false or omitted. It selects the commands menu button for private chats. After attachment, contextual commands follow in alphabetical order, including `/board` and `/jobs` when task boards are enabled, and clickable `/model` and `/reasoning` selectors for Codex; `/thinking` is not exposed. Menu registration is refreshed on every restart, so BotFather command configuration is not required. `/attach` is intentionally omitted from the menu because it requires a session ID; use the title buttons from `/sessions` instead.

## Service operation

Run in the foreground:

```sh
RUST_LOG=agentix=info agentix serve
```

Without `RUST_LOG`, `[logging].level` supplies the tracing filter. `[logging.file]` can enable a second, ANSI-free destination with a Home-relative path, `never`, `minutely`, `hourly`, or `daily` rotation, and a positive `max_files` retention count. Agentix creates the parent directory before initializing the appender. Both stderr and file logs use the computer's local RFC 3339 time.

Use the operating system's user service manager for production. Run the process as the same user that owns the coding-agent session files and Codex socket. Restrict the configuration file containing credentials to that user.

Graceful shutdown cancels channel listeners, stops inbound/event loops, checkpoints SQLite WAL state, removes live turn controls, restores detached IM menus, and sends an offline notification. Durable bindings are retained without stopping the coding-agent session. Channel adapters share one five-second shutdown deadline, so multiple adapters do not multiply the wait. Active turn text, status, owner context, and IM message references are checkpointed so a restart refreshes the existing Stop action where the host supports it and later completion edits the original message instead of creating a duplicate.

Agentix restores durable bindings and turn state before starting its control and IM tasks. Temporarily offline clients retain their bindings and do not prevent startup; the service opens its socket so extensions can reconnect. Only permanently rejected stale bindings are removed. Menu updates, online notices, and restored turn displays run in the background, so slow IM requests do not delay `Agentix is running`. Pending startup updates are cancelled before shutdown notices, and updates for bindings that changed are skipped. The running log does not mean the IM connection is ready; Telegram logs separately when initialization finishes and polling starts.

If the agent rejects a saved session because it is no longer attachable, Agentix removes that stale binding, keeps the IM detached, and reports the result.

Codex proxy registry changes wake lifecycle monitoring immediately, with a ten-second reconciliation fallback. Disconnecting a client removes its session associations; another connected owner keeps the same session live. An attached session that loses all clients suspends its binding. Reconnecting through the proxy and resuming the same session can restore the binding. Manual detach cancels that watch. Agentix's internal upstream transport reconnects separately; an app-server loaded thread alone does not establish a live CLI connection.

## Diagnostics

Startup logs include `phase` and `elapsed_ms` for the agent connection, channel/task setup, state storage, binding restoration, and background IM presentation. The `elapsed_ms` on `Agentix is running` measures service setup from opening state storage; the preceding setup phases are logged separately.

`agentix doctor` checks:

- TOML structure and selected-channel owner configuration
- required credentials in the configuration file without printing values
- global proxy URL validity (proxy connectivity is exercised when the service connects)
- state directory existence
- Codex upstream initialize/list handshake (start `serve` first to launch a missing upstream), or Pi/OMP/Claude executable checks and live registration discovery

Useful operational checks:

```sh
agentix doctor
agentix client sessions
agentix client call thread/loaded/list --params '{"limit":10}' | jq
agentix client call thread/queue/list --params '{"threadId":"019...","limit":100}' | jq
agentix client claim --ttl-minutes 10
agentix client call agentix/clients
RUST_LOG=agentix=debug agentix serve
```

`agentix client sessions` works with every configured backend and emits a normalized JSON page. `agentix client call` is Codex-specific and asks `serve` to send the supplied JSON parameters over its existing app-server connection. Diagnostic JSON is written to stdout and tracing output to stderr. If `serve` is unavailable, the client reports that it could not connect to the configured Agentix control endpoint.

Tracing timestamps use RFC 3339 in the computer's local time zone and include its UTC offset. For example, a machine configured for Asia/Shanghai emits `2026-09-04T11:42:22.975758+08:00` rather than the equivalent UTC timestamp ending in `Z`.

## Compatibility and limits

- Rust 1.95 or newer is required by the pinned Feishu SDK.
- The Codex backend requires the official standalone Codex CLI 0.153.0 or newer. Homebrew installations do not include the managed app-server layout required by Agentix.
- One service process selects one IM channel and one or more distinct backends using `[agent.codex]`, `[agent.pi]`, `[agent.omp]`, and `[agent.claude]`. Run separate config/state instances for different channels.
- Telegram converts standard Markdown in quoted agent replies to MarkdownV2 for sends and streamed edits, preserving paragraph and list boundaries inside the quote. Automatic link previews are disabled to keep structured bot responses free of unrelated webpage cards. Rendered output is conservatively bounded to 4,096 UTF-8 bytes.
- Feishu card body output is bounded before transport.
- Pi/OMP/Claude session listing queries live registered bridges using metadata-only RPCs. It does not discover offline JSONL files or spawn resumed RPC agents. Native bridging requires a Unix control endpoint.
- Codex's persistent queue API is experimental. External queue entries execute automatically, but Codex CLI 0.153.0 keeps Tab-submitted follow-ups in a private, in-process TUI queue and ignores `thread/queue/changed`. That local queue and the app-server queue used by Agentix do not synchronize or deduplicate through the official protocol. The TUI may not show an Agentix queue entry until its turn starts, and Agentix cannot list a Tab-queued TUI entry; `/queue` is authoritative only for the app-server queue. If both queues contain input when a turn ends, each owner may try to submit its next item, producing back-to-back turns with no shared ordering guarantee. Do not use both queues concurrently for the same session.
- Claude Code IM support uses the [plugin with terminal input](claude-code.md) with the existing Agentix bridge protocol. Native stop, steering, model controls, and approval relay are not advertised; terminal permissions remain local.

For the development workflow, test architecture, CI, and release process, see [Contributing to Agentix](../CONTRIBUTING.md).

### CI test cost

Plugin tests run in parallel with Rust tests on each supported operating system. CI disables dev/test debug symbols to reduce Windows linker work and cache size; local Cargo profiles are unchanged. Windows retains the workspace check, native TCP control tests, task-board tests, and three system-time-zone checks. Compare GitHub Actions step timings on equivalent revisions and cache states before claiming a speedup; the baseline Windows run `34441426541` took 17m26s, including 3m43s for workspace checking, 4m31s for the TCP test step, and 5m57s for task-board tests.

## Codex proxy verification

The reusable test suites cover the following boundaries:

| Boundary | Coverage |
| --- | --- |
| Actual `serve` startup | Active and stale Unix sockets, regular files, and occupied WS ports fail with error logs, preserve existing paths, and do not launch the upstream |
| Transport | Unix and WS listeners, WS upstream, stdio JSON lines, independent client request IDs, unchanged text frames, server requests, and streamed notifications |
| Lifecycle | Client and upstream disconnect cleanup, unsubscribe, multiple owners, socket permissions and replacement inode protection, detached upstream survival and reuse |
| Discovery | Registry-backed listing, sessions without rollout files, stale attach rejection, and PID-to-terminal association |

Run `cargo test -p agentix-codex -p agentix --lib --tests` and `cargo clippy -p agentix-codex -p agentix --all-targets -- -D warnings`. The ignored subprocess fixture is invoked by its parent integration test. Run the optional allocation-path timing comparison with `cargo test -p agentix-codex --test proxy_registry benchmark_stream_notification_observation -- --ignored --nocapture`; it has no timing threshold and is not an end-to-end throughput benchmark.
