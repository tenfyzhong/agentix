# Agentix

Agentix connects local Codex, Pi, Oh My Pi, and Claude Code sessions to Telegram, Feishu, or Slack. Attach a session from chat, send prompts, and follow the agent's replies.

## Features

- Browse local agent sessions, attach from chat, and follow streamed replies and history.
- Control Codex models, reasoning, plans, and reviews, and respond to approvals in IM.
- Restore session bindings after restarts and receive background completion notifications.
- Create Codex, Pi, OMP, and Claude Code sessions from chat with optional rmux or tmux integration.
- Coordinate work with standalone Taskix and browse project, Job, and Task boards in IM or Obsidian.
- Retain evidenced project decisions with optional Taskix memory.
- Verify Jobs before completion and synchronize Obsidian status edits with automatic rollback on failure.

## Quick start

### Install

#### macOS and Linux

Install with Homebrew:

```sh
brew tap tenfyzhong/tap
brew install agentix
```

For Codex, install the official standalone CLI (0.153.0 or newer). The Homebrew Codex package does not include the managed app-server layout Agentix requires:

```sh
curl -fsSL https://chatgpt.com/codex/install.sh | sh
```

#### Windows (x86_64)

Download and extract `agentix-<version>-x86_64-pc-windows-msvc.zip` from the [latest release](https://github.com/tenfyzhong/agentix/releases/latest), then add its directory to `PATH`. Pi/OMP live bridges can use a configured loopback TCP endpoint on Windows; Codex Unix sockets and rmux/tmux terminal delivery require macOS/Linux. See [installation options](https://github.com/tenfyzhong/agentix/wiki/Installation) for checksums and source builds.

### Configure

For a Homebrew installation, copy the example configuration:

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

1. Enable the backend you use: `[agent.codex]`, `[agent.pi]`, `[agent.omp]`, or `[agent.claude]`. The example enables Codex; each table selects its backend without a `kind` field.
2. Set `[channel].kind` to `telegram`, `feishu`, or `slack`.
3. Fill in the matching channel table: Telegram `token`, Feishu `app_id` and `app_secret`, or Slack `bot_token` and `app_token`.
4. Leave the selected channel's owner list empty for first-time claiming.

Pi/OMP need the [live-session bridge](https://github.com/tenfyzhong/agentix/wiki/Pi-and-OMP) loaded in the original terminal. Claude Code needs the [bridge plugin and terminal setup](https://github.com/tenfyzhong/agentix/wiki/Claude-Code). For Windows configuration, bot creation, and other settings, see [Getting started](https://github.com/tenfyzhong/agentix/wiki/Getting-Started) and [configuration and operations](https://github.com/tenfyzhong/agentix/wiki/Configuration-and-Operations).

### Start and claim the bot

Start Agentix and keep it running:

```sh
agentix serve
```

On Windows, use `agentix.exe serve`; run `agentix.exe doctor` from another terminal for diagnostics.

From another local terminal:

```sh
agentix client claim
```

Send the printed `/claim <code>` command to the bot in a private chat. In Slack, send `/agentix /claim <code>` in the bot DM. Skip claiming if your owner ID is already configured. A Homebrew installation can run in the background with `brew services start tenfyzhong/tap/agentix`.

### Connect and use a session

With Codex, after Agentix starts, run:

```sh
agentix doctor
codex --remote unix://
```

For Pi, OMP, or Claude Code, start the configured host with its bridge loaded and keep the original terminal running.

1. Send `/sessions` to the bot and choose **Attach** for your session.
2. Send an ordinary message to prompt the agent and follow its replies in chat.
3. Use `/last` for the latest turn, `/history` for earlier turns, and `/detach` to disconnect. Use `/exit` to request agent exit where supported (Codex requires its original CLI in the configured rmux or tmux).
4. Send `/help` for the commands supported by the current session.

Mention the bot in group chats. If another Codex process owns the session's writer, the attachment is read-only; send prompts through the original process.

## Detailed documentation

Read the [GitHub Wiki](https://github.com/tenfyzhong/agentix/wiki) for the full guides and advanced options:

- [Installation](https://github.com/tenfyzhong/agentix/wiki/Installation): Homebrew, release archives, Windows, source builds, and shell completions.
- [Configuration and operations](https://github.com/tenfyzhong/agentix/wiki/Configuration-and-Operations): backends, chat channels, startup, reload, and diagnostics.
- [Using Agentix](https://github.com/tenfyzhong/agentix/wiki/Usage): session attachment, commands, prompts, history, and queues.
- [Agent setup](https://github.com/tenfyzhong/agentix/wiki#user-content-set-up-your-environment): Pi, OMP, Claude Code, and Slack guides.
- [Taskix workflows](https://github.com/tenfyzhong/agentix/wiki#user-content-optional-taskix-workflows): host plugins, Jobs and Tasks, Obsidian, memory, and backups.

Obsidian task boards require **Obsidian 1.14 or newer** with the built-in Bases plugin. Run `taskix obsidian setup` to install Taskix Sync for status changes and bounded, scrollable boards. TaskNotes is no longer required. Recent Jobs includes all unarchived Jobs without a per-status limit.

## Development

See [CONTRIBUTING.md](CONTRIBUTING.md) for the development workflow, tests, and releases. Architecture, protocols, benchmarks, and implementation reviews are indexed in [Technical documentation](docs/README.md).
