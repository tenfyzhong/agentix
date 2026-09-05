# Agent Task Manager

One plugin package connects Codex, Claude Code, Pi, and Oh My Pi (OMP) to the independent `taskcli` task board. Lifecycle configuration is bundled; do not copy it into each project's settings.

## Bundled entrypoints

| Host | Plugin configuration | Lifecycle entrypoint |
| --- | --- | --- |
| Codex | `.codex-plugin/plugin.json` | Default discovery of `hooks/hooks.json` |
| Claude Code | `.claude-plugin/plugin.json` | Default discovery of `hooks/hooks.json` |
| Pi | `package.json` → `pi.extensions` | `extensions/pi.ts` |
| OMP | `package.json` → `omp.extensions` | `extensions/omp.ts` |

Codex and Claude share one hook file. Their manifests intentionally do not add a second hook declaration. Pi and OMP each select exactly one extension, so neither loads the other host's entrypoint. Both package manifests also include the shared `skills/` directory. The npm `files` list includes all four host manifests, hooks, extensions, runtime, skills, and this guide.

## Prerequisites and activation

Install Node.js 22+ and put `taskcli` on PATH, or set `TASKCLI_BIN` to its executable path. Initialize taskcli with your chosen document directory before enabling the plugin. Set `TASKCLI_CONFIG` if its configuration is not in the default location.

For Codex, install this directory through its plugin installation flow, enable the plugin, then use `/hooks` to review and trust its hook definitions. Installing a plugin does not bypass hook trust. Keep the default `hooks/hooks.json` in place; no user/project hook configuration needs to be merged. See [Codex bundled hooks](https://learn.chatgpt.com/docs/hooks#plugin-bundled-hooks).

For Claude Code, load the complete directory:

```sh
claude --plugin-dir /absolute/path/to/agent-task-manager
```

Review the discovered plugin hooks with `/hooks`. The shared file follows [Claude's default plugin hook location](https://code.claude.com/docs/en/plugins-reference#hooks).

For local Pi/OMP packages, install dependencies first, then use the relevant host command:

```sh
npm ci --ignore-scripts --prefix /absolute/path/to/agent-task-manager
pi install /absolute/path/to/agent-task-manager
omp plugin link /absolute/path/to/agent-task-manager
```

The last two commands are alternatives, one for each host. Restart or reload the host after installation. The [OMP package loader](https://github.com/can1357/oh-my-pi/blob/main/docs/skills/authoring-extensions.md#packagejson-manifest) selects `omp.extensions`; Pi selects `pi.extensions`. If loading an individual `.ts` file with `-e` instead, also enable the sibling skill directory.

An npm-installed copy uses `npm install --ignore-scripts` if dependencies need reinstalling: npm does not ship `package-lock.json`. Source and release copies include the lockfile and can use `npm ci`. The separate Obsidian skill is still required when an agent edits Obsidian Plan/Notes bodies.

## Lifecycle behavior

The shared Skill uses `claim → Plan → start → execute/verify → done`. Claim reserves PLANNING before the agent drafts a Plan; Plan publication requires the current lease. Start checks the Plan and dependencies and switches to EXECUTING with the same token. Pi/OMP automatically attach the lease and idempotency key to start as well as Plan writes. Codex/Claude shell calls supply them explicitly.

| Trigger | Behavior |
| --- | --- |
| Codex/Claude `SessionStart` | Restore eligible Tasks to PLANNING with a new token and inject task context |
| Codex/Claude `PreToolUse`, `PostToolUse`, `Stop` | Renew leases; `Stop` is not session exit or task completion |
| Codex/Claude `SessionEnd` | Block active Tasks owned by the ending session |
| Pi/OMP `session_start` | Restore eligible Tasks to PLANNING and start a one-minute heartbeat timer |
| Pi/OMP `before_agent_start` | Inject current task facts |
| Pi/OMP `session_shutdown` | Cancel the timer and block active Tasks |

Hook commands resolve `CLAUDE_PLUGIN_ROOT` or `PLUGIN_ROOT` inside Node, without shell-specific variable expansion. Paths containing spaces or Unicode remain one filesystem path. Pi/OMP register a structured `taskcli` tool that supplies session, executor, lease token, and idempotency identity.

Renewal and expiry apply during both planning and execution. Recovery does not require a Plan yet and never calls start automatically: the agent must repair/review the current Plan and explicitly start before continuing execution. Hooks never create or revise Plans. Registered Plan files are published through taskcli, not overwritten directly by agents.

The shared `SessionEnd` hook has a three-second timeout to respect Codex's shutdown limit; other command hooks allow 30 seconds. If shutdown times out or the process is killed, recovery falls back to the 15-minute lease expiry checked by subsequent task operations. Codex/Claude have no periodic timer, so long tool calls or idle gaps can also expire a lease. Hooks never force task completion.

## Validation

From the repository root, run `make check` with Node.js 24+ and npm. Tests inspect default hook discovery, import the manifest-selected Pi/OMP extensions, and verify the npm package file list. Cargo additionally exercises the configured commands with the compiled taskcli, one host root variable at a time, from an unrelated working directory and a plugin path containing spaces/Unicode. Linux/macOS CI exercises both sh and fish; Windows tests execute the configured command through `cmd.exe` rather than bypassing it.

To additionally exercise a Unix hook shell such as fish:

```sh
TASKCLI_TEST_HOOK_SHELL=fish cargo test -p taskcli --test cli plugin_entrypoints_execute_the_compiled_taskcli
```

These tests do not install the plugin in a user's host or invoke a live model. Native host loading, trust policy, and credentialed IM behavior remain separate acceptance checks.
