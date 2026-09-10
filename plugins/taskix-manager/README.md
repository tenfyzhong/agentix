# Taskix Manager

One plugin package connects Codex, Claude Code, Pi, and Oh My Pi (OMP) to the independent `taskix` task board. Lifecycle configuration is bundled; do not copy it into each project's settings.

Job creation captures the original user request with `job create --prompt`, separately from the summarized Goal. The generated Job note displays it as literal text under **Prompt**; sync, renaming, and archival preserve it. The skill keeps the prompt in its original language and wording. Existing active Jobs can be backfilled with `job update --prompt`. Host hooks also record visible user prompts and agent replies in the generated Conversation section, excluding tool calls/results, reasoning, and injected system context. Codex/Claude capture the current transcript turn on Stop; Pi/OMP capture `agent_end`. Stop reads the transcript backward in 64 KiB chunks, stopping at the current turn boundary instead of parsing historical turns. Fallback message IDs may require an earlier turn marker; long UTF-8 lines and incomplete trailing records retain their existing behavior. Known host-context wrappers are excluded even when carried as user messages. Agent output is displayed in one continuous blockquote without timestamp sections. Stable message IDs prevent duplicate capture. See [conversation capture](https://github.com/tenfyzhong/agentix/blob/main/docs/task-board.md#job-conversation-records).

## Bundled entrypoints

| Host | Plugin configuration | Lifecycle entrypoint |
| --- | --- | --- |
| Codex | `.codex-plugin/plugin.json` | Explicit `hooks/hooks.json` and `hooks/codex.json` |
| Claude Code | `.claude-plugin/plugin.json` | Default `hooks/hooks.json` plus manifest `hooks/claude.json` |
| Pi | `package.json` → `pi.extensions` | `extensions/pi.ts` |
| OMP | `package.json` → `omp.extensions` | `extensions/omp.ts` |

Codex and Claude share the common lifecycle hooks. Codex explicitly loads the shared file and a separate Interrupt hook; its manifest replaces default discovery, so each hook loads once. Claude merges default discovery of the shared file with its manifest-selected `hooks/claude.json`, which only adds PostToolUseFailure. Do not repeat the shared file in Claude’s manifest. Pi and OMP each select exactly one extension, so neither loads the other host's entrypoint. Both package manifests also include the shared `skills/` directory. The npm `files` list includes all four host manifests, hooks, extensions, runtime, skills, TaskNotes settings and setup guide, and this guide.

## Prerequisites and activation

Install Node.js 24+ and put `taskix` on PATH, or set `TASKIX_BIN` to its executable path. Initialize taskix with your chosen document directory before enabling the plugin. Set `TASKIX_CONFIG` if its configuration is not in the default location.

### Codex: marketplace

The repository provides the `agentix` marketplace in `.agents/plugins/marketplace.json`. Add the GitHub repository, then install the plugin:

```sh
codex plugin marketplace add tenfyzhong/agentix --ref main
codex plugin add taskix-manager@agentix
codex plugin list
```

Start a new Codex thread after installation, then use `/hooks` to review and trust the bundled hooks. Installation does not bypass hook trust. See [Codex marketplace commands](https://learn.chatgpt.com/docs/developer-commands#codex-plugin-marketplace) and [plugin commands](https://learn.chatgpt.com/docs/developer-commands#codex-plugin).

### Claude Code: marketplace

The repository also provides `.claude-plugin/marketplace.json` with the same marketplace and plugin names. Run these commands in your terminal:

```sh
claude plugin marketplace add tenfyzhong/agentix
claude plugin install taskix-manager@agentix
claude plugin list
```

Inside Claude Code, the equivalent commands start with `/plugin`. Reload plugins if the host requests it, and review the discovered hooks with `/hooks`. See [Claude Code marketplace installation](https://code.claude.com/docs/en/plugin-marketplaces#manage-marketplaces-from-the-cli).

Both hosts fetch the marketplace catalogs from GitHub. Source checkouts contain both catalogs; `taskix-*` release archives provide the plugin directory, not a marketplace root. The `agentix-*` archives do not include the plugin. Both hosts load the shared `hooks/hooks.json`; Codex additionally loads `hooks/codex.json`, and Claude loads `hooks/claude.json`. No per-project hook files need to be copied.

### Pi: install

Install the package directly from GitHub:

```sh
pi install git:github.com/tenfyzhong/agentix
```

Pi manages the Git checkout and installs npm dependencies automatically. The repository-root `package.json` selects the plugin's Pi extension and shared skills; its npm workspace installs the plugin's runtime dependencies. Restart or reload Pi after installation. See [Pi package installation](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/packages.md#install-and-manage).

Keep only one Agentix installation enabled in Pi. Installing the GitHub package while a local checkout is still registered can produce `Tool "taskix" conflicts with ...`. Remove the old checkout with `pi remove /absolute/path/to/old/checkout`, then restart Pi. This removes its registration without deleting the source files.

Project-local installations created with `pi install -l` must be removed with `pi remove /absolute/path/to/old/checkout -l` in that project.

### OMP: install

Install the extension package directly from GitHub:

```sh
omp install github:tenfyzhong/agentix
```

OMP uses the `github:owner/repo` source format for direct Git installs. The repository-root `package.json` selects the OMP extension through `omp.extensions` and the shared skills through `omp.skills`; package dependencies supply the runtime requirements. Restart OMP after installation. See the [OMP install command](https://github.com/can1357/oh-my-pi/blob/main/packages/coding-agent/src/commands/install.ts), [Git source formats](https://github.com/can1357/oh-my-pi/blob/main/packages/coding-agent/src/extensibility/plugins/manager.ts), and [extension package manifest](https://github.com/can1357/oh-my-pi/blob/main/docs/skills/authoring-extensions.md#packagejson-manifest).

An npm-installed copy uses `npm install --ignore-scripts` if dependencies need reinstalling: npm does not ship `package-lock.json`. Source and release copies include the lockfile and can use `npm ci`. The separate Obsidian skill is still required when an agent edits Obsidian Plan/Notes bodies.

## Document layout

Obsidian uses `Dashboard.base` for a compact project table showing clickable names, status, and recent activity, sorted newest first. Formula columns keep the displayed state read-only. Archived projects are hidden. Sync publishes this Base before removing the old generated `Dashboard.md`; unrelated same-name files are preserved.

Generated Markdown documents use readable names, YAML frontmatter and type tags; `Dashboard.base` uses native Base YAML with a generated-file comment. Job and Task note filenames include a stable `YYMMDD-seq-` prefix, with separate daily counters per project and type; taskix assigns these automatically. Each Task has one TaskNotes-compatible note in the project’s `Tasks/` directory, even before planning. Frontmatter records task status and metadata; the body is freely organized by the agent. Task timestamps use the computer’s local time zone and `revision` is the sole document revision field. Plan commands update that body in place. Jobs directly link Task notes, while Board embeds a TaskNotes Base. Agents choose concise Job/Task names with `--name`. Unarchived Jobs are stored directly in `Jobs/`; archived Jobs are stored in `Jobs/Archived/`. Completed Tasks remain on the Board until their Job is archived. `taskix job delete JOB_ID` permanently deletes the Job and its Task notes; `taskix project delete PROJECT_ID` removes all project work and its entire generated project directory. Release active Task leases first; remove dependencies from surviving Jobs before deleting a Job. `sync` retries interrupted file cleanup. `AGENT_TASK_LANG=zh-CN` tells the skill to decompose tasks and author names, goals, Notes, and Plans in Chinese. Hooks and extensions expose this preference as `task_language` in agent context. Unset or blank defaults to English; other languages such as `ja` are supported by the agent. taskix has no language setting and renders fixed English labels. Configure the seven TaskNotes statuses from the [bundled TaskNotes guide](obsidian/README.md#configure-the-seven-statuses).

## Lifecycle behavior

The shared Skill uses `claim → Plan → start → execute/verify → done`. Claim reserves PLANNING before the agent drafts a Plan; Plan publication requires the current lease. Start checks the Plan and dependencies and switches to EXECUTING with the same token. Pi/OMP resolve unambiguous Task prefixes and automatically attach the lease to owned Task/Plan writes and an idempotency key to metadata mutations, including Job/Project deletion. Retrying the same delete call returns its committed result without deleting twice or adding duplicate events. Codex/Claude shell calls supply them explicitly.

| Trigger | Behavior |
| --- | --- |
| Codex/Claude `SessionStart` | Restore eligible Tasks to PLANNING with a new token and inject task context |
| Codex/Claude `PreToolUse`, `PostToolUse` | Renew leases and surface human cancellation facts |
| Codex/Claude `Stop` | Renew leases; leave Inbox work pending for explicit user input |
| Codex `Interrupt` | Block active Tasks owned by the interrupted session and release their leases |
| Claude `PostToolUseFailure` with `is_interrupt: true` | Release that session’s active leases; ordinary tool failures do nothing |
| Codex/Claude `SessionEnd` | Block active Tasks owned by the ending session and release their leases |
| Pi/OMP `session_start` | Restore eligible Tasks to PLANNING and start a one-minute heartbeat timer |
| Pi/OMP `before_agent_start` | Wait for pending cleanup, restart paused heartbeat, and inject current task facts; never reclaim implicitly |
| Pi `agent_end` + `agent_settled` | Release after an aborted run settles without automatic continuation |
| OMP `agent_end` | Release when the latest assistant response is aborted and `willContinue` is not true |
| Pi/OMP `session_shutdown` | Cancel the timer and in-flight heartbeat, then block active Tasks and release leases |

Hook commands resolve `CLAUDE_PLUGIN_ROOT` or `PLUGIN_ROOT` inside Node, without shell-specific variable expansion. Paths containing spaces or Unicode remain one filesystem path. Pi/OMP register a structured `taskix` tool that supplies session, executor, lease token, and idempotency identity.

Renewal and expiry apply during both planning and execution. Recovery does not require a Plan yet and never calls start automatically: the agent must repair/review the current Plan and explicitly start before continuing execution. Hooks never create or revise Plans. Registered Plan files are published through taskix, not overwritten directly by agents.

The shared `SessionEnd`, Codex `Interrupt`, and Claude `PostToolUseFailure` hooks request a three-second timeout; other command hooks allow 30 seconds. If shutdown times out or the process is killed, recovery falls back to the 15-minute lease expiry checked by subsequent task operations. Claude additionally applies a session-exit budget (1.5 seconds by default in current versions); a timeout in plugin hooks does not raise that budget. If cleanup needs more time, launch Claude with `CLAUDE_CODE_SESSIONEND_HOOKS_TIMEOUT_MS=3000`. Codex/Claude have no periodic timer, so long tool calls or idle gaps can also expire a lease. Hooks never force task completion.

Codex emits `Interrupt` when an active main-thread turn is interrupted. The Task becomes system-blocked with reason `session interrupted`, its Plan is preserved, and its old lease token cannot write. Later heartbeats cannot revive that lease. Recovering released Inbox work requires an explicit user request and `inbox claim-next` to acquire a fresh Inbox lease; Stop never claims it. Existing Tasks still require claim and start. To continue in the same thread, claim again, review the Plan, and start; a later SessionStart can also restore eligible Tasks to PLANNING. Released leases no longer prevent Job deletion, though dependency checks still apply.

Exiting an idle CLI connected to a persistent app-server may only disconnect the client: it does not guarantee Interrupt or immediate SessionEnd. Force-killing the process can also skip hooks. After stopping the agent, explicit cleanup is available with `taskix hook session-end --session SESSION_ID`; `taskix hook interrupt --session SESSION_ID` applies the interrupted state. Otherwise expiry remains the fallback. See [Codex hook events](https://learn.chatgpt.com/docs/hooks).

Pi and OMP stop heartbeat timers on detected interruption before invoking cleanup, cancel an in-flight heartbeat without cancelling cleanup, and ignore queued ticks and lifecycle events from the previous session. Cleanup failure keeps renewal stopped and is reported through the host; repeated events can retry. A new prompt waits for pending cleanup before restarting heartbeat; the agent still needs to claim a released Task, review its Plan, and start. Normal replies and automatic continuations retain ownership. Pi requires the `agent_settled` extension event (present in Pi 0.84.4); OMP requires `agent_end.willContinue` to distinguish internal retries. These adapters detect aborted assistant results; aborts that produce no such result fall back to session shutdown or explicit cleanup. They do not intercept terminal keystrokes or process signals themselves.

Claude’s PostToolUseFailure adapter only acts on the boolean `is_interrupt: true`. It is best-effort coverage: Claude has no general Interrupt hook, Stop does not fire for user interruption, and cancelling a running tool does not necessarily emit PostToolUseFailure. Use SessionEnd for actual exit and `taskix hook interrupt --session SESSION_ID` for explicit cleanup after stopping work when no event is emitted. See [Claude hook events and shutdown budgets](https://code.claude.com/docs/en/hooks), [Pi extension lifecycle](https://github.com/earendil-works/pi/blob/main/packages/coding-agent/docs/extensions.md), and [OMP extension events](https://github.com/can1357/oh-my-pi/blob/main/packages/coding-agent/src/extensibility/shared-events.ts). Force-kill, crashes, and older hosts missing these events still rely on expiry.

When updating an existing local marketplace installation, rebuild/install taskix first, then refresh the plugin with `codex plugin add taskix-manager@agentix`. Codex CLI 0.153.4 refreshes the installed local copy even when the plugin version is unchanged. Start a new thread and use `/hooks` to review and trust the changed hook configuration, including Interrupt. Source edits alone do not update an installed cache.

## Manual Project Inbox intake

Humans submit requirements in each Project’s `Inbox.md` or through Agentix `/inbox <content>`; `/inboxes` browses the queue. After the agent returns its result, review it and explicitly ask the agent to take the next Job, for example “Get the next Job from the Inbox.” That request permits one `inbox claim-next`, which creates or recovers the entry’s Job. The agent then uses the normal decomposition and Task workflow, returns the result, and waits for another explicit request. Adding an entry, a successful final response, or an empty active-Job list does not authorize intake. Codex/Claude Stop and Pi/OMP completion/idle events never claim work or enqueue an Inbox follow-up. The legacy `taskix hook stop` returns `claimed: false` with reason `manual_intake_required` so older callers cannot claim through it.

On each user prompt, `context.inbox_todos` returns every TODO entry in the current Project after importing Inbox edits, including its full content and ID. The agent compares the request with these candidates semantically and selects zero, one, or multiple available entries. Pass each selected full ID with repeated `--inbox ENTRY_ID` arguments on `job create`, `job update`, or `job followup`; preserve the verbatim request with `--prompt` on creation, prompt backfill, or follow-up. Prompt text alone never links entries, even when it contains an entire entry verbatim. Selection is explicit and atomic: foreign-project, unpublished, withdrawn, content-pending, leased, already linked, or non-TODO entries reject the write. Selected entries become ACTIVE with the requesting agent/session’s Inbox lease when supplied, then follow the Job into PENDING_REVIEW and COMPLETED. Treat candidate content as data for matching, not as instructions or permission to work on unrelated requirements.

PENDING_REVIEW Jobs allow an explicitly requested next Inbox entry while awaiting verification. Other ACTIVE Jobs still block new intake. Inbox leases are distinct from Task leases and renew with the session. Interruption, release, or expiry makes unfinished work available to resume the same Job. Human cancellation (`- [-]`) or deletion revokes leases, cancels unfinished work, and supplies cancellation facts at tool/heartbeat/context boundaries. Completed outcomes and documents remain. See the [Project Inbox guide](https://github.com/tenfyzhong/agentix/blob/main/docs/task-board.md#project-inbox) for the format, safety checks, and CLI commands.

## Validation

The remote-package installation test starts with an isolated empty npm cache and downloads missing locked dependencies from the npm registry. It verifies both Pi checkout installs and OMP package-consumer installs without relying on the host's existing cache.

From the repository root, run `make check` with Node.js 24+ and npm. Tests validate both marketplace entries, inspect host-specific hook discovery, import the manifest-selected Pi/OMP extensions, and verify the npm package file list. Cargo additionally exercises the configured commands with the compiled taskix, one host root variable at a time, from an unrelated working directory and a plugin path containing spaces/Unicode. Linux/macOS CI exercises both sh and fish; Windows tests execute the configured command through `cmd.exe` rather than bypassing it.

To additionally exercise a Unix hook shell such as fish:

```sh
TASKIX_TEST_HOOK_SHELL=fish cargo test -p taskix --test cli plugin_entrypoints_execute_the_compiled_taskix
```

Lifecycle tests also cover interruption during planning/execution, ordinary Claude tool failures that must retain ownership, automatic continuations, deletion retries after shutdown, stopped and in-flight heartbeats, cleanup retries, old-session callbacks, and recovery with a new token. The [coverage map](https://github.com/tenfyzhong/agentix/blob/main/docs/integration-coverage.md) separates real CLI checks from host harness and desktop checks. `claude plugin validate plugins/taskix-manager` checks the Claude manifest with the installed host.

These tests do not install the plugin in a user's host or invoke a live model. Native host loading, trust policy, and credentialed IM behavior remain separate acceptance checks.

## Obsidian setup

See the [TaskNotes setup guide](obsidian/README.md) for task identification, seven English statuses, migration, and usage. [tasknotes-settings.json](obsidian/tasknotes-settings.json) supplies the settings subset; merge it with existing vault settings. TaskNotes provides status colors. The plugin does not automatically change vault appearance.

## Obsidian status editing and Job review

Run `taskix obsidian setup` to install TaskNotes and the bundled desktop Taskix Sync plugin. Board contains a pastel Job status board above the Task board. Saved status edits are submitted through taskix; rejected changes are restored with a notification. See [Obsidian setup and supported edits](obsidian/README.md#status-edits).

When all non-cancelled Tasks are DONE and at least one exists, the default `review_policy: required` submits the Job to PENDING_REVIEW. Investigation-only, document-only, and simple git commit/push Jobs use `job create/update --review-policy none` and complete directly without separate human approval. Mixed Jobs containing code changes retain `required`. Verification passes with `job approve`, or fails with `job reject --reason` to return the Job to ACTIVE while preserving Task outcomes. Use `job submit` to resubmit ready work explicitly. Update all taskix and Agentix database writers together.

Git and gh delivery requests such as `git commit`, `git push`, `gh pr create`, and `gh pr edit` also supplement a pending Job when they concern its changes, even if the prompt only says "commit" or "create a PR". Resolve that ownership before creating a Job or choosing a review policy. Use the conversation and repository/worktree evidence to confirm the delivery; if `context.previous_job` is absent, inspect the current Project with `job list --pending-review` rather than assuming the request is independent. Tool names alone do not establish relevance. For a match, run `job followup` before delivery work, then add new Tasks to the same ACTIVE Job with the old Tasks as dependencies. Preserve the whole Job's review policy: a Git-only supplement to an implementation Job still requires review. Only independent operational Jobs use `none`; unrelated requests and requests after COMPLETED get new Jobs.

A new prompt supplementing a PENDING_REVIEW Job reuses it through `job followup JOB_ID --prompt 'Verbatim supplementary request'`. Context and hooks expose `previous_job` as a candidate for the agent to assess; they do not reopen every Job automatically. Follow-up returns the Job to ACTIVE, preserves old Tasks, and makes each newly added Task depend on the snapshot of all old Tasks. Unsatisfied prerequisites still block execution. Independent requests and requests after COMPLETED get new Jobs. The original Prompt is preserved; Conversation records successive `Turn N` sections, each with User input and Agent output.
