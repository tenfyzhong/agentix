# Agentix

Agentix connects local Codex, Pi, Oh My Pi, and Claude Code sessions to Telegram, Feishu, or Slack. Attach a session from chat, send prompts, and follow the agent's replies.

## Features

- Browse local agent sessions, attach from chat, and follow streamed replies and history.
- Control Codex models, reasoning, plans, and reviews, and respond to approvals in IM.
- Restore session bindings after restarts and receive background completion notifications.
- Create Codex, Pi, OMP, and Claude Code sessions from chat with optional rmux integration.
- Coordinate work with standalone `taskix` and browse project, Job, and Task boards in IM or Obsidian.
- Verify Jobs before completion and synchronize Obsidian status edits with automatic rollback on failure.

Install Taskix Manager from GitHub using the [host-specific installation guide](plugins/taskix-manager/README.md#prerequisites-and-activation). Codex and Claude Code use the `agentix` marketplace; Pi and OMP install the repository as an extension package with their own entrypoints and shared runtime dependencies.

Claude Code IM access uses the [Agentix bridge plugin](docs/claude-code.md), installed from the same `agentix` marketplace.

## Install

### macOS and Linux

```sh
brew tap tenfyzhong/tap
brew install agentix
```

For Codex, install the official standalone CLI (0.153.0 or newer). The Homebrew Codex package does not include the managed app-server layout Agentix requires:

```sh
curl -fsSL https://chatgpt.com/codex/install.sh | sh
```

### Windows (x86_64)

Download and extract `agentix-<version>-x86_64-pc-windows-msvc.zip` from the [latest release](https://github.com/tenfyzhong/agentix/releases/latest), then add the extracted directory to `PATH`. Pi/OMP live bridges can use a configured loopback TCP endpoint on Windows. Codex Unix sockets and rmux terminal delivery require macOS/Linux; standalone taskix and task plugins also support Windows.

For checksums, other release archives, or building from source, see the [installation guide](docs/guide.md#install).

## Configure

Copy the example configuration. With Homebrew:

```sh
mkdir -p ~/.config/agentix
cp "$(brew --prefix agentix)/share/agentix/agentix.example.toml" ~/.config/agentix/config.toml
chmod 600 ~/.config/agentix/config.toml
```

On Windows, assuming the archive was extracted into `agentix`:

```powershell
New-Item -ItemType Directory -Force "$HOME\.config\agentix" | Out-Null
Copy-Item .\agentix\agentix.example.toml "$HOME\.config\agentix\config.toml"
```

Edit `~/.config/agentix/config.toml`:

1. Enable one or more of `[agent.codex]`, `[agent.pi]`, `[agent.omp]`, and `[agent.claude]`; each table selects its backend without a `kind` field. For Pi/OMP, install the [live-session bridge](plugins/agentix-bridge/README.md) in the original terminal. For Claude Code, install the [bridge plugin](docs/claude-code.md); the default non-Channel mode requires installing rmux and starting Claude inside an rmux terminal.
2. Select `telegram`, `feishu`, or `slack` in `[channel].kind`.
3. Fill in the Telegram bot `token`, the Feishu `app_id` and `app_secret`, or Slack `bot_token` and `app_token`, in the matching channel table.
4. Leave the selected channel's owner list empty for first-time claiming.

For Slack, follow [Slack setup](docs/slack.md). For Feishu bot setup, proxies, and other settings, see [Configuration and operations](docs/development-and-operations.md).

## Start

Check the configuration and start Agentix:

```sh
agentix doctor
agentix serve
```

On Windows, use `agentix.exe doctor` and `agentix.exe serve`. To run a Homebrew installation in the background, use `brew services start tenfyzhong/tap/agentix`.

Before starting a managed Codex daemon, Agentix loads the exported environment from the current user's login shell, so Homebrew services pass configured paths and other variables to Codex. Put fish environment settings outside `status is-interactive` blocks. See [login shell environment](docs/development-and-operations.md#login-shell-environment) for overrides and existing daemons.

Keep the service running and claim the bot from another local terminal:

```sh
agentix client claim
```

Send the printed `/claim <code>` command to the bot in a private chat. Skip this step if your owner ID is already configured.

## Pi and OMP

Install the extensions, then restart the host:

```sh
pi install git:github.com/tenfyzhong/agentix
omp install github:tenfyzhong/agentix
```

Configure `[agent.pi]` or `[agent.omp]`, start `agentix serve`, and keep the original terminal session running. The extension connects to `~/.local/share/agentix/control.sock`; select it with `/sessions pi` or `/sessions omp`. `/detach` leaves the terminal running. See [bridge setup](plugins/agentix-bridge/README.md) for multiple backends and custom endpoints. Live bridging supports Unix sockets on macOS/Linux and loopback TCP endpoints, including Windows.

## Claude Code

From the Agentix checkout, install the bridge plugin:

```sh
claude plugin marketplace add .
claude plugin install agentix-bridge@agentix
```

Add the backend to your Agentix configuration:

```toml
[agent.claude]
command = "claude"
session_dir = "~/.claude/projects"
```

Start or restart the current Agentix build. In an rmux terminal, change to your project directory and launch Claude:

```sh
claude
```

Keep the terminal running and idle, and select the session with `/sessions claude` in IM. The plugin clears any terminal draft before sending IM prompts through rmux and reports replies through hooks; Channel flags are not required, including when using third-party API providers. `/rmux claude` creates a suitable terminal from IM. Channel delivery remains available through explicit configuration. See the [Claude startup guide](docs/claude-code.md#start-claude-code) for local build commands, setup, and terminal delivery limitations.

## Basic use

1. Start a coding-agent session locally.
2. Send `/sessions` to the bot and select the session's **Attach** action.
3. Send an ordinary message to prompt the agent and receive its replies in chat.
4. Use `/history` to browse turns, `/stop` to interrupt a writable active turn, and `/detach` to disconnect.
5. Send `/help` to see the commands available in the current session.

Mention the bot in group chats. If another Codex process owns the session's writer, Agentix attaches read-only; send prompts through that original process.

To create sessions from chat, install optional rmux and use `/rmux` (or `/rmux pi`, `/rmux omp`, `/rmux codex`); see [rmux workspaces](docs/usage.md#rmux-workspaces). Connecting to existing sessions does not require rmux. Pi and OMP attachments control the original process through the shared, owner-only Unix control socket.

With [task boards configured](docs/guide.md#im-task-boards), use `/dashboard`, `/board`, and `/jobs` to browse work. Use `/inboxes` to view the current project's human queue and `/inbox <content>` to append a requirement; explicitly ask the agent to take the next Job after reviewing its current result. See [Project inbox](docs/task-board.md#project-inbox) for document submission and cancellation.

For Obsidian task views, configure taskix for your vault and run [`taskix obsidian setup`](docs/task-board.md#obsidian-plugin-setup) to install and configure TaskNotes and automatically reload the vault through Obsidian CLI.

Run `taskix --help` to browse commands and `taskix <command> --help` to see each group's subcommands and descriptions. For a specific operation, use nested help such as `taskix task claim --help` or `taskix plan create --help`.

See the [detailed guide](docs/guide.md) for installation alternatives, shell completions, session behavior, task boards, and links to the command reference and development documentation.
