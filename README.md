# Agentix

Agentix connects the coding agents already running on your computer to Telegram or Feishu, so you can monitor and continue local sessions when you step away from the terminal or IDE. It is a local-first Rust bridge for Codex, Pi, and Oh My Pi. Claude Code is intentionally out of scope for this release.

Each IM conversation maps explicitly and durably to an agent session. Messages include a readable session title, short session ID, and turn identifier so concurrent sessions remain unambiguous.

## Capabilities

- Native Codex app-server integration plus isolated Pi and Oh My Pi RPC transports
- Telegram long polling and Feishu long-connection support with interactive actions
- A duplex FIFO message center for IM traffic, with ordered retries at the outbound queue head
- A global HTTP/HTTPS/SOCKS5 proxy configured in TOML, including for Homebrew services
- Running-session discovery, attachment, history, prompts, queues, steering, stopping, approvals, and user-input round trips
- Codex controls for models, reasoning, Fast mode, plans, goals, reviews, diffs, forks, compaction, skills, and MCP servers
- Interactive rmux workspace browsing and safe Codex session creation from IM
- Owner allowlists, one-time owner claiming, group mention requirements, event deduplication, and single-use actions
- Durable bindings, restart recovery, process-exit notifications, and automatic Codex reattachment
- Streamed in-place responses, background completion notifications, and reply context

While `agentix serve` is running, Agentix checks running Codex sessions for completed turns every ten seconds using read-only history queries, including sessions that have never been attached or were detached from IM. Background monitoring does not resume sessions or acquire their writer locks. New completions include the completed turn's prompt and response, a Background label, and an Attach button. Feishu uses a purple header and a tinted quote area; Telegram uses a ⚫ Background marker and blockquotes. Notifications go to authenticated IM conversations known to the service. Send the bot `/help` once to register a conversation for these notifications; attaching a session is optional.

Before a Codex session's first user message, background history reads may report that the thread is not materialized yet. Agentix logs this expected condition at debug level and keeps polling; other background read errors remain warnings.

Attaching a session restores its latest turn with a Stop button when that turn is running. Only the current attached session's active turn message has Stop; switching sessions, moving the attachment to another conversation, detaching, or finishing the turn removes it from the previous message. Copies shown by `/history` never include Stop.

Startup recovery, automatic reattachment, and shutdown notifications only use channels enabled in the current configuration. Saved bindings and turn messages for other channels are retained for when those channels are enabled again. Each IM adapter and its clones share a duplex FIFO message center. Normalized incoming messages use an independent inbound queue; sends, edits, menus, owner-claim replies, callback acknowledgements, and Feishu reply lookups use the outbound queue. A rate-limited request stays at the head until it succeeds, fails permanently, or is cancelled, so later requests cannot overtake its retries. Telegram honors `retry_after` and spaces requests globally and per chat. Feishu HTTP 429 responses use exponential backoff from one second up to 60 seconds because its SDK does not expose the server retry delay. Cancelling a request removes that operation while preserving the channel cooldown. Telegram streams and working-duration updates refresh at most once every five seconds. Final turn updates bypass that refresh interval while still respecting Telegram cooldowns. Telegram rate-limit logs include the API method and chat ID.

To disable completion notices for unattached sessions, add this to `config.toml` and restart Agentix:

```toml
[notifications]
background_turns = false
```

The default is `true`. Disabling notifications also stops automatic background turn polling and full-content reads. When no sessions need exit/resume monitoring, automatic session discovery stops too. Existing attached or draining turn cards still complete in place; attached-session exit/resume monitoring remains active. Both completion deduplication caches keep only the latest completed turn per session, with recipient tracking for that turn, so records do not accumulate for every completed turn.

## Quick start

### Install

#### macOS and Linux

Install Agentix with Homebrew:

```sh
brew tap tenfyzhong/tap
brew install agentix
```

The Homebrew formula is maintained in [tenfyzhong/homebrew-tap](https://github.com/tenfyzhong/homebrew-tap/blob/main/Formula/agentix.rb). Release automation updates that formula and publishes a macOS arm64 bottle.

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

Keep the extracted directory in a stable location and add it to your user `PATH`. Windows supports the Pi and Oh My Pi backends; the Codex backend is not available on Windows because its transport requires a Unix-domain socket.

#### Build from source

Install the Rust toolchain declared by `rust-toolchain.toml`, then run:

```sh
make release
```

The binary is written to `target/release/agentix` (`agentix.exe` on Windows).

### Shell completions

Generate completions for the installed CLI with `agentix completions bash`,
`agentix completions zsh`, or `agentix completions fish`. This command does not
require a configuration file or a running server.

For bash, add this line to `~/.bashrc` (or `~/.bash_profile` on macOS):

```bash
source <(agentix completions bash)
```

For zsh, save the completion file:

```zsh
mkdir -p ~/.zsh/completions
agentix completions zsh > ~/.zsh/completions/_agentix
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
```

Restart your shell after installation. Regenerate saved files after upgrading
Agentix. Source checkouts and release archives also include ready-to-use files
in `completions/`: `agentix.bash`, `_agentix`, and `agentix.fish`. You can source
the bash file or copy the zsh/fish file to the directories above.

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

1. Select one backend in `[agent]`: `codex`, `pi`, or `oh-my-pi`.
2. Select one IM transport with `[channel].kind`: `telegram` or `feishu`.
3. Configure the matching `[channel.telegram]` or `[channel.feishu]` table.
4. Leave the selected channel's owner list empty for first-time claiming, or add the owner IDs directly.

For Codex, keep the standalone binary path explicit:

```toml
[agent]
kind = "codex"
command = "~/.codex/packages/standalone/current/codex"
endpoint = "unix://"
```

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

Use your proxy's actual address and port. HTTP, HTTPS, SOCKS5, and SOCKS5h proxies are supported. The setting covers all Telegram requests and works without shell proxy variables. The Feishu SDK does not use this setting and retains its existing network behavior. After changing it, restart a running Homebrew service with `brew services restart tenfyzhong/tap/agentix`.

See [Configuration and operations](docs/development-and-operations.md) for backend details, Feishu permissions, logging, service management, and diagnostics.

### Start

Validate the configuration, then start the bridge:

```sh
agentix doctor
agentix serve
```

On Windows, use `agentix.exe doctor` and `agentix.exe serve`. A Homebrew installation can run in the background instead:

```sh
brew services start tenfyzhong/tap/agentix
```

For a source build that is not on `PATH`, replace `agentix` with `target/release/agentix` in the commands above.

If the selected channel has no configured owner, keep `agentix serve` running, execute `agentix client claim` in another local terminal, and send the printed `/claim <code>` command to the bot in a private chat.

## Development

Run `make check` to check formatting, run Clippy, and execute the workspace tests. Channel shutdown deadline tests use Tokio's paused clock to verify the shared grace period and task cancellation independently of database and filesystem latency. Service lifecycle tests also exercise startup and shutdown with a temporary SQLite database.

After changing CLI commands or options, run `make completions` and commit the
updated files. Tests verify that the checked-in completions match the CLI.

CI uses Rust 1.95.0. Ensure `cargo`, `rustc`, `cargo-clippy`, and `rustfmt` all come from that toolchain rather than mixing Homebrew and rustup installations. The test suite checks the non-Unix Codex compatibility API on Unix hosts as well, so Windows-only API omissions are caught locally.

## Documentation

- [Usage guide](docs/usage.md)
- [Configuration and operations](docs/development-and-operations.md)
- [Contributing](CONTRIBUTING.md)
- [Product design](docs/product-design.md)
- [Internal architecture and message flow diagrams](docs/architecture.md): service components, duplex FIFO queues, and rate-limit retry ordering
