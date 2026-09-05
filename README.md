# Agentix

Agentix connects the coding agents already running on your computer to Telegram or Feishu, so you can monitor and continue local sessions when you step away from the terminal or IDE. It is a local-first Rust bridge for Codex, Pi, and Oh My Pi. Claude Code can use the standalone task plugin; its IM transport is outside this release.

Each IM conversation maps explicitly and durably to an agent session. Messages include a readable session title, short session ID, and turn identifier so concurrent sessions remain unambiguous.

## Capabilities

- Native Codex app-server integration plus isolated Pi and Oh My Pi RPC transports
- Telegram long polling and Feishu long-connection support with interactive actions
- Running-session discovery, attachment, history, prompts, queues, steering, stopping, approvals, and user-input round trips
- Codex controls for models, reasoning, Fast mode, plans, goals, reviews, diffs, forks, compaction, skills, and MCP servers
- Interactive rmux workspace browsing and safe Codex session creation from IM
- Owner allowlists, one-time owner claiming, group mention requirements, event deduplication, and single-use actions
- Durable bindings, restart recovery, process-exit notifications, and automatic Codex reattachment
- Streamed in-place responses, background completion notifications, and reply context
- Standalone `taskcli`: SQLite jobs, concurrent task claims, versioned plans, audit events, and read-only Obsidian/Markdown boards
- Optional IM task controls and a shared Codex, Claude, Pi, and OMP plugin, with stable interfaces for future Agent Team orchestration

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

The binaries are written to `target/release/agentix` and `target/release/taskcli` (`.exe` on Windows).

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

## Documentation

- [Usage guide](docs/usage.md)
- [Configuration and operations](docs/development-and-operations.md)
- [Contributing](CONTRIBUTING.md)
- [Product design](docs/product-design.md)
- [Architecture](docs/architecture.md)
- [Task board, standalone CLI, and agent plugin](docs/task-board.md)

## Task board

`taskcli` works independently of the IM bridge. Choose an existing output directory explicitly:

```sh
taskcli init --format markdown --root /absolute/path/to/documents --directory "Agent Tasks"
# Or use an Obsidian vault root:
taskcli init --format obsidian --root /absolute/path/to/vault --directory "Agent Tasks"
taskcli project register
taskcli job create --title "Deliver a feature" --goal "Acceptance checks"
```

One Git repository stays one Project across worktrees and time; each independent requirement gets its own Job. Different Jobs and their Tasks can run concurrently. Members claim individual Tasks with fenced leases; a future Team tool can attach its identifier and maintain shared context keyed by Job ID.

The workflow is `claim → Plan → start → execute/verify → done`. Claim reserves planning ownership before any Plan is written; start checks the Plan and dependencies without replacing the lease. Both phases appear in the existing `IN_PROGRESS` column, and hooks renew/recover planning leases too. Only the current lease holder can create or revise a Plan.

Task state changes go through CLI commands or Agentix IM actions. Generated boards are logically read-only, use `[[wikilinks]]` in Obsidian or relative Markdown links elsewhere, and require no Kanban/Tasks plugin or file watcher. See the [task board guide](docs/task-board.md) for plans, claiming, plugin installation, recovery, and IM configuration.

For the design rationale, read [task decomposition, Skill, and Hook mechanisms](docs/task-workflow-mechanisms.md), covering responsibility boundaries, ownership, concurrency, recovery, and future Agent Team integration.

The [agent-task-manager plugin](plugins/agent-task-manager/README.md) bundles Codex/Claude lifecycle hooks and manifest-selected Pi/OMP extensions. Enable the package in the chosen host and review its hooks; no per-project hook configuration needs to be copied.

`make check` covers concurrent CLI processes, recovery after a committed write is interrupted, the Task state/command matrix, real plugin-to-CLI calls, and task actions through both IM adapters. It requires Node.js 24+ and npm in addition to Rust. An opt-in desktop test checks actual Obsidian rendering and link navigation; see [validation and remaining live-system checks](docs/task-board.md#validation).
