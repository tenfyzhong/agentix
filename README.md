# Agentix

Agentix connects the coding agents already running on your computer to Telegram or Feishu, so you can monitor and continue local sessions when you step away from the terminal or IDE. It is a local-first Rust bridge for Codex, Pi, and Oh My Pi. Claude Code can use the standalone task plugin; its IM transport is outside this release.

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
- Standalone `taskcli`: SQLite jobs with dependencies displayed as Mermaid graphs showing seven task statuses in a light palette shared with TaskNotes and clickable note links, concurrent task claims with lease release on supported host interruptions and session shutdown, task notes tagged `task` and `agent/task` with prerequisite and revision metadata, audit events, a compact clickable project Dashboard (Obsidian Base or Markdown table), and generated TaskNotes boards containing project metadata in Obsidian or Markdown directories
- Optional IM task controls and a shared Codex, Claude, Pi, and OMP plugin, with stable interfaces for future Agent Team orchestration

While `agentix serve` is running, Agentix checks running Codex sessions for completed turns every ten seconds using read-only history queries, including sessions that have never been attached or were detached from IM. Background monitoring does not resume sessions or acquire their writer locks. New completions include the completed turn's prompt and response, a Background label, and an Attach button. Feishu uses a purple header and a tinted quote area; Telegram uses a ⚫ Background marker and blockquotes. Notifications go to authenticated IM conversations known to the service. Send the bot `/help` once to register a conversation for these notifications; attaching a session is optional.

Before a Codex session's first user message, background history reads may report that the thread is not materialized yet. Agentix logs this expected condition at debug level and keeps polling; other background read errors remain warnings.

Attaching a session restores its latest turn with a Stop button when that turn is running and writable. If another Codex process owns the session's writer, Agentix connects read-only, restores the latest saved content, and checks for updates every ten seconds. With process discovery enabled, read-only attachments also report process exit and reappearance; reappearance preserves read-only access without acquiring the writer. Detaching stops their lifecycle monitoring. The menu keeps history and navigation commands; sending prompts and changing the session require the original Codex process. Session lists infer external-session activity from the latest saved turn when live status is unavailable. Other attachment failures show their reason and a fresh Retry attach action.

Only the current attached session's writable active turn message has Stop; switching sessions, moving the attachment to another conversation, detaching, or finishing the turn removes it from the previous message. Copies shown by `/history` never include Stop.

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

#### Separate release archives

Each [GitHub release](https://github.com/tenfyzhong/agentix/releases/latest) publishes Agentix and taskcli separately, using the same version and targets (macOS arm64, Linux x86_64/arm64, and Windows x86_64):

- `agentix-<version>-<target>.tar.gz`: Agentix, its example configuration, and its shell completions.
- `taskcli-<version>-<target>.tar.gz`: taskcli, its example configuration, its shell completions, task documentation, and `plugins/agent-task-manager/`.

Both archives include `README.md` and `LICENSE`. Windows also has `.zip` archives for each tool. The shared `SHA256SUMS` covers both tools and all archive formats. Verify the downloaded archive against its matching checksum before extracting it, then add the extracted binary's directory to `PATH`.

Download the taskcli archive to use the task board independently; Agentix is not required. Download both archives if you need both tools. Keep the taskcli plugin directory at a stable path and follow the [plugin activation guide](plugins/agent-task-manager/README.md).

#### Build from source

Install the Rust toolchain declared by `rust-toolchain.toml`, then run:

```sh
make release
```

The binaries are written to `target/release/agentix` and `target/release/taskcli` (`.exe` on Windows).

### Shell completions

Both CLIs support `completions bash`, `completions zsh`, and `completions fish`.
For example, use `agentix completions bash` or `taskcli completions bash`.
Generation requires no configuration, running server, or task database.

For bash, add this line to `~/.bashrc` (or `~/.bash_profile` on macOS):

```bash
source <(agentix completions bash)
source <(taskcli completions bash)
```

For zsh, save the completion file:

```zsh
mkdir -p ~/.zsh/completions
agentix completions zsh > ~/.zsh/completions/_agentix
taskcli completions zsh > ~/.zsh/completions/_taskcli
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
taskcli completions fish > ~/.config/fish/completions/taskcli.fish
```

Restart your shell after installation. Regenerate saved files after upgrading
either CLI. Source checkouts include ready-to-use files
in `completions/`: `agentix.bash`, `_agentix`, `agentix.fish`, `taskcli.bash`,
`_taskcli`, and `taskcli.fish`. Each release archive includes only its own CLI's
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

## IM task boards

Task document cleanup preserves unrelated files at destinations where projection failed. Authored Plan frontmatter supports LF and CRLF delimiters and quoted YAML keys. Pi/OMP lease injection accepts full Task IDs and unambiguous Task prefixes.

Add the following to `~/.config/agentix/config.toml`, then restart Agentix to browse work in Telegram or Feishu:

```toml
[task_board]
config = "~/.config/taskcli/config.toml"
```

The referenced taskcli configuration must already exist. Agentix opens the database specified there and creates it if missing; use the same configuration as your taskcli writers to see their existing work. Restart Agentix after changing this section. Without it, board commands report `Task board is not configured.`

| Command | View |
| --- | --- |
| `/dashboard` | Project dashboard; click a project to open its task board |
| `/board` | The attached session's task board; click a task for its Markdown details |
| `/jobs` | All Jobs associated with the attached session; click a Job for its Markdown details |

`/dashboard` is registered in Telegram’s default menu at startup when the task board is configured. Top-level commands appear in the order `/sessions`, `/dashboard`, `/cancel`, `/rmux`, `/help`; contextual commands follow in alphabetical order. `/board` and `/jobs` are contextual secondary commands added after attach and removed on detach. Both follow the current attachment, including Jobs containing tasks whose lease or last recorded session matches it. Sibling tasks show overall Job progress; blocked and completed work remains visible after lease release. Archived projects and Jobs are excluded from lists.

Job details display the authored Goal and Notes, plus buttons for associated tasks. Task details display the authored Task note body, status and planning/execution phase, with a **Job** button to open the parent Job. Existing Task action buttons remain available where ownership permits. Lists and long Markdown details, including Task reasons, have **Previous**/**Next** buttons; code fences remain balanced across detail pages. Detail headers shorten long titles to 60 characters; their full titles remain in the paginated body. Project boards include a **Dashboard** button, and Job details link to their **Project board**.

Browsing reads current task data without changing task state or Plan hashes. Navigation buttons are scoped to the conversation, owner, and current attachment; reopen the command after switching sessions. The earlier `/projects` and `/sessionboard` commands are replaced by `/dashboard` and `/board`. Legacy `/tasks [job-or-project]` and `/task <id>` remain direct shortcuts. See the [task board guide](docs/task-board.md#agentix-integration) for setup and task actions.

## Development

Run `make check` to check formatting, run Clippy, and execute the workspace tests. Channel shutdown deadline tests use Tokio's paused clock to verify the shared grace period and task cancellation independently of database and filesystem latency. Service lifecycle tests also exercise startup and shutdown with a temporary SQLite database.

After changing CLI commands or options, run `make completions` and commit the
updated files for both CLIs. Tests verify that the checked-in completions match
their CLI and that taskcli generation does not read configuration or create task state.

CI uses Rust 1.95.0. Ensure `cargo`, `rustc`, `cargo-clippy`, and `rustfmt` all come from that toolchain rather than mixing Homebrew and rustup installations. The test suite checks the non-Unix Codex compatibility API on Unix hosts as well, so Windows-only API omissions are caught locally.

## Documentation

- [Usage guide](docs/usage.md)
- [Configuration and operations](docs/development-and-operations.md)
- [Contributing](CONTRIBUTING.md)
- [Product design](docs/product-design.md)
- [Internal architecture and message flow diagrams](docs/architecture.md): service components, duplex FIFO queues, and rate-limit retry ordering
- [Task board, standalone CLI, and agent plugin](docs/task-board.md)
- [Task workflow responsibilities and lifecycle](docs/task-workflow-mechanisms.md)
- [Integration coverage and live acceptance boundaries](docs/integration-coverage.md)
