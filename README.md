# Agentix

Agentix connects local Codex, Pi, and Oh My Pi sessions to Telegram or Feishu. Attach a session from chat, send prompts, and follow the agent's replies.

## Install

### macOS and Linux

```sh
brew tap tenfyzhong/tap
brew install agentix
```

For Codex, install the official standalone CLI (0.153.0 or newer). Agentix requires its managed app-server layout:

```sh
curl -fsSL https://chatgpt.com/codex/install.sh | sh
```

### Windows (x86_64)

Download and extract `agentix-<version>-x86_64-pc-windows-msvc.zip` from the [latest release](https://github.com/tenfyzhong/agentix/releases/latest), then add the extracted directory to `PATH`. Use Pi or Oh My Pi; the Codex backend is not available on Windows.

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

1. Select `codex`, `pi`, or `oh-my-pi` in `[agent]` and set its executable path using the examples in the file.
2. Select `telegram` or `feishu` in `[channel].kind`.
3. Fill in the Telegram bot `token`, or the Feishu `app_id` and `app_secret`, in the matching channel table.
4. Leave the selected channel's owner list empty for first-time claiming.

For Feishu bot setup, proxies, and other settings, see [Configuration and operations](docs/development-and-operations.md).

## Start

Check the configuration and start Agentix:

```sh
agentix doctor
agentix serve
```

On Windows, use `agentix.exe doctor` and `agentix.exe serve`. To run a Homebrew installation in the background, use `brew services start tenfyzhong/tap/agentix`.

Keep the service running and claim the bot from another local terminal:

```sh
agentix client claim
```

Send the printed `/claim <code>` command to the bot in a private chat. Skip this step if your owner ID is already configured.

## Basic use

1. Start a coding-agent session locally.
2. Send `/sessions` to the bot and select the session's **Attach** action.
3. Send an ordinary message to prompt the agent and receive its replies in chat.
4. Use `/history` to browse turns, `/stop` to interrupt a writable active turn, and `/detach` to disconnect.
5. Send `/help` to see the commands available in the current session.

Mention the bot in group chats. If another Codex process owns the session's writer, Agentix attaches read-only; send prompts through that original process.

With [task boards configured](docs/guide.md#im-task-boards), use `/dashboard` to browse projects, or `/board` and `/jobs` to view the attached session's work. Task and Job details link to each other.

See the [detailed guide](docs/guide.md) for installation alternatives, shell completions, session behavior, task boards, and links to the command reference and development documentation.
