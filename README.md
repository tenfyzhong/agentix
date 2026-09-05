# Agentix

Agentix connects the coding agents already running on your computer to Telegram or Feishu, so you can monitor and continue local sessions when you step away from the terminal or IDE. It is a local-first Rust bridge for Codex, Pi, and Oh My Pi. Claude Code is intentionally out of scope for this release.

Each IM conversation maps explicitly and durably to an agent session. Messages include a readable session title, short session ID, and turn identifier so concurrent sessions remain unambiguous.

## Capabilities

- Native Codex app-server integration plus isolated Pi and Oh My Pi RPC transports
- Telegram long polling and Feishu long-connection support with interactive actions
- A global HTTP/HTTPS/SOCKS5 proxy configured in TOML, including for Homebrew services
- Running-session discovery, attachment, history, prompts, queues, steering, stopping, approvals, and user-input round trips
- Codex controls for models, reasoning, Fast mode, plans, goals, reviews, diffs, forks, compaction, skills, and MCP servers
- Interactive rmux workspace browsing and safe Codex session creation from IM
- Owner allowlists, one-time owner claiming, group mention requirements, event deduplication, and single-use actions
- Durable bindings, restart recovery, process-exit notifications, and automatic Codex reattachment
- Streamed in-place responses, background completion notifications, and reply context

While `agentix serve` is running, Agentix checks running Codex sessions for completed turns every two seconds using read-only history queries, including sessions that have never been attached or were detached from IM. Background monitoring does not resume sessions or acquire their writer locks. New completions include the completed turn's prompt and response, a Background label, and an Attach button. Feishu uses a purple header and a tinted quote area; Telegram uses a ⚫ Background marker and blockquotes. Notifications go to authenticated IM conversations known to the service. Send the bot `/help` once to register a conversation for these notifications; attaching a session is optional.

Startup recovery, automatic reattachment, and shutdown notifications only use channels enabled in the current configuration. Saved bindings and turn messages for other channels are retained for when those channels are enabled again. Telegram requests that return `retry_after` wait for the specified delay and retry automatically, including restored turn updates and command menus.

To disable completion notices for unattached sessions, add this to `config.toml` and restart Agentix:

```toml
[notifications]
background_turns = false
```

The default is `true`. Existing attached or draining turn cards still complete in place.

## Quick start

### Install

#### macOS and Linux

Install Agentix with Homebrew:

```sh
brew tap tenfyzhong/tap
brew install agentix
```

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

## Documentation

- [Usage guide](docs/usage.md)
- [Configuration and operations](docs/development-and-operations.md)
- [Contributing](CONTRIBUTING.md)
- [Product design](docs/product-design.md)
- [Architecture](docs/architecture.md)
