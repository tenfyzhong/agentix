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

### Codex: marketplace

The repository provides the `agentix` marketplace in `.agents/plugins/marketplace.json`. From a checkout containing that catalog, add the repository or worktree root, then install the plugin:

```sh
codex plugin marketplace add /absolute/path/to/agentix
codex plugin add agent-task-manager@agentix
codex plugin list
```

Use the repository root, not `plugins/agent-task-manager`, as the marketplace path. Start a new Codex thread after installation, then use `/hooks` to review and trust the bundled hooks. Installation does not bypass hook trust. See [Codex marketplace commands](https://learn.chatgpt.com/docs/developer-commands#codex-plugin-marketplace) and [plugin commands](https://learn.chatgpt.com/docs/developer-commands#codex-plugin).

### Claude Code: marketplace

The repository also provides `.claude-plugin/marketplace.json` with the same marketplace and plugin names. Run these commands in your terminal:

```sh
claude plugin marketplace add /absolute/path/to/agentix
claude plugin install agent-task-manager@agentix
claude plugin list
```

Inside Claude Code, the equivalent commands start with `/plugin`. Reload plugins if the host requests it, and review the discovered hooks with `/hooks`. See [Claude Code marketplace installation](https://code.claude.com/docs/en/plugin-marketplaces#manage-marketplaces-from-the-cli).

For either host, once these catalogs are published on the repository's default branch, you can replace the local marketplace path with `tenfyzhong/agentix`. Until then, use the checkout containing the catalogs. Source checkouts contain both catalogs; `taskcli-*` release archives provide the plugin directory, not a marketplace root. The `agentix-*` archives do not include the plugin. Both hosts load the same `hooks/hooks.json`; no per-project hook files need to be copied.

### Pi: install

Use the complete plugin directory from a source checkout or extracted taskcli release archive. Install its dependencies, then register it with Pi:

```sh
npm ci --ignore-scripts --prefix /absolute/path/to/agent-task-manager
pi install /absolute/path/to/agent-task-manager
```

The local package stays at that path; Pi reads `pi.extensions` and `pi.skills` from `package.json`. Restart or reload Pi after installation. See [Pi package installation](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/packages.md#install-and-manage).

### OMP: install

Use OMP's `install` command for the complete package:

```sh
npm ci --ignore-scripts --prefix /absolute/path/to/agent-task-manager
omp install /absolute/path/to/agent-task-manager
```

OMP links local packages and reads `omp.extensions` and `omp.skills`. Keep the package at a stable path and restart OMP after installation. See the [OMP install command](https://github.com/can1357/oh-my-pi/blob/main/packages/coding-agent/src/commands/install.ts) and [package loader](https://github.com/can1357/oh-my-pi/blob/main/docs/skills/authoring-extensions.md#packagejson-manifest). Use a version whose `omp install --help` lists local paths as supported targets.

An npm-installed copy uses `npm install --ignore-scripts` if dependencies need reinstalling: npm does not ship `package-lock.json`. Source and release copies include the lockfile and can use `npm ci`. The separate Obsidian skill is still required when an agent edits Obsidian Plan/Notes bodies.

## Document layout

Generated documents use readable names, YAML frontmatter and type tags. Job and Task note filenames include a stable `YYMMDD-seq-` prefix, with separate daily counters per project and type; taskcli assigns these automatically. Each Task has one TaskNotes-compatible note in the project’s `Tasks/` directory, even before planning. Frontmatter records task status and metadata; the body is freely organized by the agent. Task timestamps use the computer’s local time zone and `revision` is the sole document revision field. Plan commands update that body in place. Jobs directly link Task notes, while Board embeds a TaskNotes Base. Agents choose concise Job/Task names with `--name`. Unarchived Jobs are stored directly in `Jobs/`; archived Jobs are stored in `Jobs/Archived/`. Completed Tasks remain on the Board until their Job is archived. `taskcli job delete JOB_ID` permanently deletes the Job and its Task notes; `taskcli project delete PROJECT_ID` removes all project work and its entire generated project directory. Release active Task leases first; remove dependencies from surviving Jobs before deleting a Job. `sync` retries interrupted file cleanup. `AGENT_TASK_LANG=zh-CN` tells the skill to decompose tasks and author names, goals, Notes, and Plans in Chinese. Hooks and extensions expose this preference as `task_language` in agent context. Unset or blank defaults to English; other languages such as `ja` are supported by the agent. taskcli has no language setting and renders fixed English labels. Configure the seven TaskNotes statuses from the [task board guide](../../docs/task-board.md#obsidian-plugin-setup).

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

From the repository root, run `make check` with Node.js 24+ and npm. Tests validate both marketplace entries, inspect default hook discovery, import the manifest-selected Pi/OMP extensions, and verify the npm package file list. Cargo additionally exercises the configured commands with the compiled taskcli, one host root variable at a time, from an unrelated working directory and a plugin path containing spaces/Unicode. Linux/macOS CI exercises both sh and fish; Windows tests execute the configured command through `cmd.exe` rather than bypassing it.

To additionally exercise a Unix hook shell such as fish:

```sh
TASKCLI_TEST_HOOK_SHELL=fish cargo test -p taskcli --test cli plugin_entrypoints_execute_the_compiled_taskcli
```

These tests do not install the plugin in a user's host or invoke a live model. Native host loading, trust policy, and credentialed IM behavior remain separate acceptance checks.

## Obsidian setup

See the [TaskNotes setup guide](obsidian/README.md) for task identification, seven English statuses, migration, and usage. [tasknotes-settings.json](obsidian/tasknotes-settings.json) supplies the settings subset; merge it with existing vault settings. TaskNotes provides status colors. The plugin does not automatically change vault appearance.
