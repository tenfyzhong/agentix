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

## User documentation

**Read the [GitHub Wiki](https://github.com/tenfyzhong/agentix/wiki) for installation, configuration, and everyday use.** New users should start with [Getting started](https://github.com/tenfyzhong/agentix/wiki/Getting-Started).

- [Installation](https://github.com/tenfyzhong/agentix/wiki/Installation): Homebrew, release archives, Windows, source builds, and shell completions.
- [Configuration and operations](https://github.com/tenfyzhong/agentix/wiki/Configuration-and-Operations): backends, chat channels, startup, reload, and diagnostics.
- [Using Agentix](https://github.com/tenfyzhong/agentix/wiki/Usage): session attachment, commands, prompts, history, and queues.
- [Agent setup](https://github.com/tenfyzhong/agentix/wiki#user-content-set-up-your-environment): Pi, OMP, Claude Code, and Slack guides.
- [Taskix workflows](https://github.com/tenfyzhong/agentix/wiki#user-content-optional-taskix-workflows): host plugins, Jobs and Tasks, Obsidian, memory, and backups.

## Development

See [CONTRIBUTING.md](CONTRIBUTING.md) for the development workflow, tests, and releases. Architecture, protocols, benchmarks, and implementation reviews are indexed in [Technical documentation](docs/README.md).
