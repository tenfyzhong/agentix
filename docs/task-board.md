# Task board and standalone CLI

`agentix-task` supplies SQLite coordination and document projection; `taskcli` is its independent command-line interface. Agentix optionally uses the same library and database for IM control. Agent Team membership, scheduling, and shared context are external concerns, associated through stable Job IDs and optional `delegated_by` metadata.

## Model and concurrency

A Project identifies a Git repository or stable directory. Git worktrees share their repository's common directory and reuse one Project. Jobs represent independently acceptable requirements, while Tasks are executable steps. New requirements after delivery create new Jobs; completed and cancelled Jobs can be manually archived into `Jobs/Archived/`. Unarchived Jobs, including completed ones, remain directly in `Jobs/`. There is no milestone layer or Job-level exclusive lock.

Tasks use `TODO`, `IN_PROGRESS`, `BLOCKED`, `WAITING_USER`, `DONE`, `FAILED`, and `CANCELLED`. IN_PROGRESS has two phases: claim enters `PLANNING`; explicit start enters `EXECUTING`. Only EXECUTING can finish with done. Both phases can fail, block, wait, release, or cancel. TODO can be claimed, blocked, put into WAITING_USER, or cancelled. BLOCKED and WAITING_USER can switch between each other, return to PLANNING through claim, fail, or cancel. FAILED requires `retry`; DONE/CANCELLED require `reopen`. Outside IN_PROGRESS, `phase` is null. Reopening a prerequisite after a downstream Task started executing is rejected; planning alone does not freeze dependencies.

Jobs aggregate to COMPLETED once at least one non-cancelled Task exists and all such Tasks are DONE. Cancelling every Task does not count as delivering the requirement. Completed Jobs reject additional Tasks. Reopen corrects a Task's result and returns its Job to ACTIVE; new scope belongs in a new Job. Active Jobs cannot be archived. Job cancellation requires its active Task leases to be released first.

Each Task has at most one effective lease, and each executor/session pair has at most one Task. Claim atomically checks state and ownership, reserving the Task before an agent drafts its Plan; it does not require a Plan or completed dependencies. Plan creation/revision requires the active session and lease token. Start checks ownership, a nonblank current Plan file, and DONE dependencies before switching to EXECUTING under the same lease. Plan writes and start validation share an output lock; state/lease checks run again in the database transaction. `started_at` records the first execution start, not claim time. Different Tasks and Jobs can plan or execute concurrently. Dependencies must remain inside one Project and cannot form cycles or change after execution starts. Leases coordinate task facts, not working directories: code Tasks need their own branches/worktrees and dependencies for shared resources.

SQLite uses WAL, a ten-second busy timeout, short immediate write transactions, foreign keys, revision checks, and optional idempotency keys. Entity documents are stored as typed JSON rows with relational generated columns and indexes; dependency edges and events have their own tables. Mutations load the local task graph inside a write transaction to validate invariants and persist only changed entities. This first version targets local repositories and modest task graphs on one computer, not shared database files on network filesystems.

Database schema v7 moves Plan files into Task notes, retains durable deletion cleanup and document sequence counters, and migrates earlier databases to flat `Jobs/` and `Jobs/Archived/` directories, dated and numbered Job and Task Plan filenames, readable project names, project archival, lifecycle timestamps, and one current Plan per Task. Existing Jobs and Tasks receive daily sequence numbers in creation order (ID breaks timestamp ties), independently per project and entity type. The migration preserves IDs, leases, editable Goal/Notes, and the latest Plan body. Old Plan files are removed only after the replacement documents are written; empty old directories are removed too. Back up SQLite and documents together before upgrading all writers. Older binaries reject v7. Configuration and JSON envelope `schema_version` remain 1; database `PRAGMA user_version` is separate.

## Shell completions

`taskcli completions bash`, `taskcli completions zsh`, and `taskcli completions fish` print shell scripts directly, including when `--json` is present. Generation skips configuration loading and does not open or mutate the task database, so it works before `taskcli init`.

Source checkouts and `taskcli-*` release archives include `completions/taskcli.bash`, `completions/_taskcli`, and `completions/taskcli.fish`. Follow the [shell installation instructions](../README.md#shell-completions). Contributors regenerate all Agentix and taskcli scripts with `make completions`; tests compare the generated output with these files and exercise nested commands, options, formats, and file paths.

## Configuration

We recommend Obsidian for day-to-day task management. Use an existing vault, enable the TaskNotes and Bases plugins, and select `--format obsidian` for rendered boards, task queries, and wikilink navigation. See [Obsidian plugin setup](#obsidian-plugin-setup). Choose `--format markdown` when you prefer a plain directory or another editor; storage and CLI workflows remain supported, but generic Markdown viewers do not render the plugin views.

```sh
# Recommended:
taskcli init --format obsidian --root /existing/vault --directory "Agent Tasks"
# Alternative: run this instead of the Obsidian initialization above.
# taskcli init --format markdown --root /existing/documents --directory "Agent Tasks"
```

Run only the applicable initialization. Default config: `~/.config/taskcli/config.toml`; override with `--config` or `TASKCLI_CONFIG`. Initialization refuses to overwrite an existing config. `--database` selects the independent task database. The config has this shape:

```toml
schema_version = 1

[storage]
path = "~/.local/share/taskcli/tasks.sqlite3"

[documents]
format = "obsidian"
root = "/existing/vault"
directory = "Agent Tasks"
```

The format is mandatory. The root must be an existing absolute directory, and Obsidian roots must contain `.obsidian`. The output subdirectory is relative, has no traversal components, and may be `.`. The database must be outside the output directory and separate from Agentix runtime storage. Task databases carry a SQLite application identifier; taskcli rejects unrelated databases, and Agentix refuses to add its runtime tables to task databases. Symlinks cannot make document paths escape their configured root. Keep one output configuration per database; configuration relocation is an explicit migration, not automatic synchronization.

## Workflow

```sh
taskcli project register                       # Run in the Git worktree
taskcli project register --root /work/docs --name Docs
taskcli job create --project prj_ID --title "New requirement" --goal "Acceptance checks"
taskcli task add --job job_ID --title "Implement and verify the storage layer"
taskcli task add --job job_ID --title "Integrate the client"
taskcli task depend task_CLIENT task_STORAGE
taskcli task claim task_STORAGE --executor agent:member --session HOST_SESSION --json
# After claim succeeds, draft the Plan and publish it with the returned token:
taskcli plan create task_STORAGE --file /work/storage-plan.md --session HOST_SESSION --lease-token lease_TOKEN
taskcli task start task_STORAGE --session HOST_SESSION --lease-token lease_TOKEN
# Execute the Plan, verify acceptance, then call done with the same token.
```

IDs use UUIDv7 with `prj_`, `job_`, `task_`, and `plan_` prefixes; full IDs or unambiguous prefixes are accepted. `project show` also accepts an unambiguous project name. Outside Git, pass `--project` explicitly. `task list --ready` discovers TODO work with completed dependencies; `task list --status TODO` also includes work that can be planned before dependencies finish. Claim before drafting either kind of Task. Register all known initial Tasks and dependency edges before implementation, verify their generated notes, and prepare each detailed Plan when taking up its Task so Job completion reflects the agreed scope.

Claim returns the Task and a `lease` containing its token. Subsequent writes to a leased Task must include the current session and token:

```sh
taskcli task heartbeat task_ID --session HOST_SESSION --lease-token lease_TOKEN
taskcli task done task_ID --session HOST_SESSION --lease-token lease_TOKEN
taskcli task block task_ID --reason "Upstream unavailable" --session HOST_SESSION --lease-token lease_TOKEN
taskcli task wait task_ID --reason "Need a decision" --session HOST_SESSION --lease-token lease_TOKEN
taskcli task fail task_ID --reason "Acceptance test failed" --session HOST_SESSION --lease-token lease_TOKEN
taskcli task release task_ID --reason "Handing off" --session HOST_SESSION --lease-token lease_TOKEN
taskcli task retry task_ID
taskcli task reopen task_ID
```

A lease lasts 15 minutes. Renew at least once a minute during planning and execution. Terminal, blocked, waiting, and release operations remove the lease and clear the phase. Abnormal exit hooks and expired leases create a system BLOCKED reason. Expiry is checked on CLI/library operations and Agentix refresh, without a standalone background daemon. Resuming the same session reacquires only system-blocked Tasks that have not been taken over; it issues new tokens and returns to PLANNING. Manual blocks stay blocked. Missing Plans do not prevent planning recovery; repair/review the Plan and explicitly call start before continuing execution. Hooks never automatically start or finish work.

Use `--expect-revision N` to protect an update based on an earlier read. Use `--idempotency-key KEY` to retry identical requests without duplicate entities/events; reuse with different arguments fails. Local CLI access is not a Team authorization boundary. Lease tokens fence stale executions, while operating-system access controls protect the local files.

`job update`, `task update`, `task depend/undepend`, `plan revise`, `job cancel`, `job archive/unarchive`, `job delete`, and `project delete` provide the remaining mutations. Consult `--help` for each command.

```sh
taskcli job list --active
taskcli job list --completed
taskcli job list --archived --period 2026-09
taskcli job list --created-from 2026-07-01 --created-to 2026-09-30
taskcli event list --job job_ID --after 0 --limit 100 --json
taskcli context --session HOST_SESSION --json
```

Timestamp fields are Unix seconds in UTC; creation date filters include both specified calendar dates. JSON responses carry `schema_version: 1`, `ok`, and either `result` or `error`. Mutation responses also include `sequence` and `projection_pending`. Exit status is 0 on success, 1 on business/runtime failure, and 2 on argument errors. Event listing returns ordered events and `next_cursor`; the limit is 1–1000. Event payloads contain no Plan body.

## Read-only document projections

```text
Dashboard.base  # Obsidian; Dashboard.md in Markdown mode
Projects/<project-name>/
  Board.md
  Jobs/YYMMDD-seq-<job-name>.md
  Jobs/Archived/YYMMDD-seq-<job-name>.md
  Tasks/YYMMDD-seq-<task-name>.md
```

Unarchived Jobs live directly under `Jobs/`; only explicitly archived Jobs move into `Jobs/Archived/`. Archiving and restoring a Job preserve its filename. Migration removes empty legacy `Jobs/Active/` and `Jobs/Archive/YYYY/MM/` directories after publishing the replacement documents and links.

In Obsidian mode, `Dashboard.base` is a compact native Bases table with Name, Status, and Updated columns, sorted by recent activity. Each project name links directly to its Board. Filters include only generated, active project Boards in the configured output directory; archived projects are hidden. Formula columns display database-derived values without making project state editable in the table. Markdown mode renders the same fields in `Dashboard.md` as a portable table. Neither view lists individual Jobs or Tasks. Sync publishes the replacement before removing the old registered Dashboard; an unrelated existing `Dashboard.base` is preserved and reported as a conflict. Switching formats migrates the Dashboard in either direction.

Names preserve Unicode and spaces. IDs stay in YAML frontmatter, not filenames. Only collisions add `-2`, `-3`, etc.; comparison is case insensitive. `job create --name` and `task add --name` accept a concise summary separately from the full `--title`. Names default to a portable, at most 48-character title. Agents should summarize the work when choosing `--name`, rather than rely on truncation. `job update --name` and `task update --name` also work on completed work and update generated links without adding Plan versions.

Job and Task Plan filenames begin with `YYMMDD-seq-`, for example `260905-0001-Implement login.md`. The date is the Job or Task creation date in UTC, even if its Plan is created later. Sequence numbers start at 1 each day, independently for Jobs and Tasks in each project (Tasks across Jobs share the project counter), and use at least four digits. Allocation is transactional; archived or cancelled work keeps its number. Renaming and Plan revisions preserve the prefix, and display names remain concise. The `sequence` property is stored with Job and Task metadata and the Task’s Plan frontmatter.

Generated Markdown documents have YAML frontmatter containing an ID, creation time, and a type tag. `Dashboard.base` contains native Base YAML with a generated-file comment instead of Markdown frontmatter. Job properties include IDs, sequence, status, revision, and creation/update/start/completion/cancellation/archive times. They omit `document_path`, `title`, `name`, and embedded `task`/`tasks` fields; the Job heading and Task note links remain in the document body. Task properties include `revision` and lifecycle times, without a `version` field. Task timestamps are ISO 8601 in the computer’s local time zone, with the offset for each timestamp; other document timestamps remain UTC. CLI JSON timestamps remain Unix seconds. Project `Board.md` also provides `updated_at`, derived from the latest project creation/archive or Job/Task update timestamp; syncing alone does not advance it. It records the repository root, Git remote, revision, archive state, `sync_status`, and `sync_sequence`. Board is the project note and also embeds its task view. Its ID is the Project ID. Sync migrates generated `meta.md` information into Board, updates Dashboard and task project links to Board, and deletes the old managed meta file after publishing the replacement documents. Board has no separate Project link.

| Document | Tags |
| --- | --- |
| Job | `agent/job` |
| Archived Job | `agent/archived/job` |
| Task (including its Plan) | `agent/task`, `task` |
| Project Board | `agent/project`, `agent/board` |
| Markdown Dashboard | `agent/dashboard` |

Each Task has one TaskNotes-compatible note in `Tasks/`, created even before a Plan is published. Notes carry `task` for TaskNotes identification and `agent/task` for Agentix board filtering. Sync adds missing tags to existing notes and preserves custom tags. Its frontmatter contains the Task ID, optional Plan ID, state, phase, revision, local dates, project link, Job link, and `dependencies`: a list of prerequisite Task IDs, or `[]`. Register all known initial Tasks and dependencies before implementation. When taking up a Task, claim it and publish its freely structured Plan into that same note. `plan revise` updates it in place and advances the Task revision. The document exposes only `revision`; the internal Plan publication counter remains part of CLI metadata. Authored properties are merged with managed metadata. Dependency fields are generated from SQLite, refreshed by sync, and cannot be overridden by authored Plan properties; `task start` requires all prerequisites to be DONE.

Job task sections directly link Task notes, displaying their filenames without `.md`. Obsidian uses wikilinks; Markdown uses ordinary relative links. Board embeds a TaskNotes Base over the project's task notes, rather than duplicating checkbox entries. Completed and cancelled work remains visible until its Job or Project is archived. Goal, Notes, names, and Plan prose are preserved as authored.

Each nonempty Job task section also includes a generated Mermaid dependency graph. Every Task in the Job appears, including independent Tasks; arrows point from prerequisite to dependent Task. Direct prerequisites from other Jobs appear once with their Job name, without expanding those Jobs' full dependency graphs. Task additions, renames, and dependency changes refresh the graph automatically; `taskcli sync` adds it to existing Job documents. The graph is read-only and uses the same database dependencies as the Task notes. Task links below it remain available in both document formats. Dependencies are displayed in the graph, without a duplicate `Dependencies:` list under each Task; sync removes those generated lists from existing Jobs while preserving authored Goal and Notes content.

Graph nodes show `Task name · STATUS` using the seven `TaskStatus` values: `TODO`, `IN_PROGRESS`, `BLOCKED`, `WAITING_USER`, `DONE`, `FAILED`, and `CANCELLED`. Their light background colors match the bundled [TaskNotes status configuration](../plugins/agent-task-manager/obsidian/tasknotes-settings.json), with dark text for contrast; only `DONE` means successful completion. Planning and executing are phases within `IN_PROGRESS`, not additional statuses. Status changes through taskcli refresh the graph, including cross-Job prerequisite nodes.

Clicking a node's label opens its Task note. Obsidian output uses an HTML internal link inside the node with a vault-relative file path, supporting normal navigation and hover previews without changing Mermaid security settings. Markdown output uses a relative file link. Renames and Job archival regenerate the target paths. Mermaid viewers that disable interactive links can still use the ordinary Task links below the graph. Nodes display database state at the last projection or sync; they do not embed editable TaskNotes widgets or read customized vault colors.

### Task language

Task language is a skill preference, configured with `AGENT_TASK_LANG` in the agent host environment. It controls task decomposition, Job/Task titles and concise names, goals, Notes, and Plan prose. The default is English (`en`) when unset or blank; `zh-CN` selects Chinese, and other languages such as `ja` are passed to the agent without a CLI allowlist. Explicit language instructions for the current work take precedence.

```fish
set -Ux AGENT_TASK_LANG zh-CN
```

For a macOS desktop host launched outside fish, set the environment before restarting that host:

```sh
launchctl setenv AGENT_TASK_LANG zh-CN
```

Codex/Claude hooks and Pi/OMP extensions include `task_language` in injected agent context. This field belongs to the plugin, not `taskcli context`. taskcli has no language option, ignores `AGENT_TASK_LANG` and the obsolete `TASKCLI_LANGUAGE`, and uses fixed English labels for generated sections. Supplied names and prose remain unchanged; changing the skill preference does not translate existing documents.

When upgrading, rename the host environment variable to `AGENT_TASK_LANG`, remove `TASKCLI_LANGUAGE`, and remove `[documents].language` from taskcli configuration. The config loader tolerates and ignores that legacy key so existing installations still open; new configs and CLI context no longer include it. Restart desktop hosts to inherit the new environment.

### Obsidian plugin setup

Enable [TaskNotes](https://tasknotes.dev/obsidian/core-concepts/) and Obsidian's Bases core plugin. Set TaskNotes' identification tag to `task`, retain its default field mapping, and configure the seven exact status values: `TODO`, `IN_PROGRESS`, `BLOCKED`, `WAITING_USER`, `DONE`, `FAILED`, and `CANCELLED`. Only DONE is successful completion; disable automatic archival for these statuses. See the [TaskNotes setup guide](../plugins/agent-task-manager/obsidian/README.md) for labels, colors, settings, and examples.

`Board.md` embeds a `tasknotesKanban` Base grouped by status, with an explicit seven-column order and empty columns visible. The Board filters by the exact Task folder, project ID, `agent/task` tag, and `archived != true`. TaskNotes reads task frontmatter, so Job links and authored checklists do not create duplicate cards. Open Board in Reading view or Live Preview; the old Kanban plugins and Tasks queries are no longer used.

After updating all writers, run `taskcli sync`. Schema 7 migrates registered Plan paths to `Tasks/`; sync preserves authored content and properties and removes old managed files after publishing replacements. Board and Job paths remain stable. Task notes use the Task ID as `id`, and keep the Plan ID separately in `plan_id`. New document creation does not open tabs. taskcli does not change vault-wide plugin settings. Sync removes the obsolete managed `Tasks.md` list and its navigation links; Board is the project’s only status view.

### Project archival

```sh
taskcli project archive PROJECT_ID
taskcli project list --archived
taskcli project unarchive PROJECT_ID
```

Complete or cancel all Jobs before archiving a Project. An archived Project is hidden from the Dashboard and the default project list, keeps its documents and history, and rejects new work until restored. Job archive/unarchive remains independent; project unarchive does not unarchive individual Jobs.

### Deleting work

```sh
taskcli job delete JOB_ID
taskcli project delete PROJECT_ID
```

`job delete` permanently removes the Job record and document, its Tasks, dependencies, and Plan records/files, including archived work. Other Jobs and their documents remain. `project delete` removes every Job, Task, and Plan belonging to the Project, then removes its entire `Projects/<project-name>/` directory, including manually added notes, hidden files, and attachments. Its repository directory is outside this cleanup scope. Neither command requires prior archival or completion.

Release active Task leases before deleting. Job deletion rejects dependencies from Tasks in surviving Jobs; remove those dependencies explicitly first. Dependencies wholly within a deleted Project are removed together. Both commands accept `--expect-revision` and `--idempotency-key`; no interactive prompt is added. Audit events and idempotency results remain in SQLite; unfiltered `event list` includes deletion events. Deleted Job/Task filename sequence numbers are not reused within the surviving Project.

Database removal and file-cleanup records commit in one transaction. File failures return `projection_pending`; fix the reported issue and run `taskcli sync`, or retry the exact delete request with the same idempotency key. Cleanup survives process restarts. A Project name whose directory is still pending deletion cannot be registered until cleanup completes. Cleanup refuses paths redirected through symlinks, and removes nested attachment symlinks without following their targets.

### Read-only boundary

Generated regions are logically read-only, not filesystem-protected. Manual edits to status, title, dependencies, or ordering are overwritten by the next projection and never imported. There is no `watch` command. Goal/Notes markers preserve their editable bodies. Explicit `job update --goal` replaces a manually edited Goal; Notes remain untouched. Missing/duplicated editable markers fail synchronization instead of dropping content.

TaskNotes provides card actions, property editing, and board settings. The generated views are logically read-only, but these plugin controls are not locked: dragging cards or editing task properties can still change Markdown temporarily. They never claim, start, or complete a task in SQLite. Use an agent/taskcli for state changes and run `taskcli sync` to repair an accidental plugin edit. Strict UI-level prevention would require an additional integration; taskcli does not claim to provide it.

Task notes contain authoritative Plan bodies. Authored YAML properties and tags merge with managed properties into a single frontmatter block; `plan show` returns `properties` separately from `body`. Agents must claim first, then publish through `plan create` or `plan revise` with the current lease. Publishing a revision updates the Task revision and replaces the same file; do not directly overwrite registered Plans. Agents authoring Obsidian bodies must load the available Obsidian skill and use `[[wikilinks]]`; use a session-specific temporary draft when necessary and let taskcli publish the registered file after checking ownership. Other directories use standard Markdown. Raw filesystem writes cannot be lease-fenced: manual edits are detected by hash refresh during `sync` and `plan show`, not prevented by SQLite. Do not use those edits as a concurrent agent workflow.

Database commits happen before generated-document updates. A filesystem failure returns success with `projection_pending` so callers do not recreate committed work. `taskcli sync` repairs the projection. Output file locks serialize independent CLI processes; temporary-file replacement protects each document. Archival writes the destination and updates managed links before removing the previous generated file. A Plan replacement body is committed with its lease check and retained until projection acknowledges it, so interrupted publication can be retried. Back up the database and document tree together; editable bodies are not recoverable from SQLite alone.

```sh
taskcli doctor --json  # healthy, missing_plans, database/projection sequence
taskcli sync
```

### Data coverage and recovery

TaskNotes is the only community plugin required by these views; enable Obsidian's built-in Bases core plugin as well. TaskNotes indexes Markdown files, so the saved notes and board remain readable if taskcli's SQLite database is lost. Agent task execution and synchronization still depend on SQLite.

The vault is not a complete database export:

| Data | Representation in Obsidian |
| --- | --- |
| Current project and Job identity, hierarchy, lifecycle, and revision | Metadata, Job properties, filenames, links, and prose; not a serialized copy of every database field |
| Current Task identity, status, phase, revision, sequence, and lifecycle dates | Task frontmatter |
| Plan body, custom properties, Goal, and Notes | Authored note content; SQLite does not retain a complete copy after publication |
| Task dependencies | Prerequisite Task IDs in Task frontmatter `dependencies`, with navigable links in Job prose; no import contract |
| Task reasons | Displayed in Job prose; not exported in Task frontmatter |
| Task ordering and execution bookkeeping | Position, last executor/session, delegation, and system-block flag are not fully exported |
| Ownership leases | Tokens and expiration times are not exported |
| Audit history and request idempotency | Event log, request fingerprints, and saved results remain in SQLite |
| Internal Plan publication state | Publication counters, hashes, and pending unpublished bodies are not fully exported; document `revision` does not replace the internal publication counter |
| Synchronization and cleanup state | Managed-path bookkeeping, pending deletions, and sequence counters that prevent reuse of deleted filenames remain in SQLite; `Board.md` exposes sync status and sequence |

There is currently no vault import or database rebuild command. `sync` projects the database into documents; it does not reconstruct database records from existing frontmatter. Notes could support a future partial reconstruction of current work after validation, but cannot reproduce missing history, ownership, or retry records. Restored work would need fresh claims rather than recovered lease tokens.

For a restorable backup:

1. Pause agent/CLI writers and note edits, run `taskcli sync`, and confirm `taskcli doctor --json` reports a healthy projection.
2. Create a database snapshot using the [SQLite backup API](https://sqlite.org/backup.html) or the SQLite shell's `.backup` command. Do not rely on copying only the main file of a live WAL database.
3. Back up the matching document tree and taskcli configuration while writes remain paused. Include TaskNotes settings to preserve the Obsidian display configuration.
4. Restore the matched database, documents, and configuration with writers stopped. Validate the restored copy separately with `doctor`, then `sync` and `doctor` before resuming work. Recover/reclaim active work through the normal lease workflow.

If only the vault survives, preserve a copy before further taskcli writes. Existing task notes can still be browsed with TaskNotes, but normal agent execution requires a database backup or a separately implemented, explicitly partial reconstruction.

## Host plugin

The shared package is `plugins/agent-task-manager`, included in the standalone `taskcli-*` release archives, not the `agentix-*` archives. Use a taskcli archive or a source checkout for the plugin; Homebrew packaging is maintained separately in the tap. It has Codex/Claude manifests, a shared Skill, command hooks, and Pi/OMP TypeScript entrypoints. Node.js 22 or newer is required for command hooks. Put `taskcli` on PATH or set `TASKCLI_BIN`; set `TASKCLI_CONFIG` when using a non-default config.

Install through the repository's `agentix` marketplace in Codex and Claude Code. Add the repository/worktree root as the marketplace, then install `agent-task-manager@agentix`. Codex uses `codex plugin marketplace add` followed by `codex plugin add`; Claude Code uses `claude plugin marketplace add` followed by `claude plugin install`. The catalogs are `.agents/plugins/marketplace.json` and `.claude-plugin/marketplace.json`. See the [complete installation commands](../plugins/agent-task-manager/README.md#prerequisites-and-activation), including when GitHub-based installation is available.

Claude merges default discovery of the shared hook file with manifest-selected `hooks/claude.json` for explicitly interrupted tool failures. Codex explicitly loads that file plus `hooks/codex.json` for Interrupt; its manifest replaces default discovery, avoiding duplicate hooks. Review/enable hooks in the host as required; Codex requires reviewing and trusting plugin hooks through `/hooks`. The command resolves plugin-root environment variables inside Node rather than using shell-specific expansion.

The package explicitly includes its manifests, hooks, extensions, runtime, skills, and activation guide in npm distributions. Pi and OMP each select their own entrypoint through `package.json`; installing the complete package does not require copying hooks into project settings. See the [plugin activation and lifecycle guide](../plugins/agent-task-manager/README.md).

For Pi/OMP, install dependencies and use the host's `install` command on the complete plugin directory from a source checkout or taskcli release archive:

```sh
npm ci --ignore-scripts --prefix /absolute/path/to/agent-task-manager
pi install /absolute/path/to/agent-task-manager
omp install /absolute/path/to/agent-task-manager
```

Run only the install command for your chosen host, then restart or reload it. Both hosts load the selected extension and the shared Skill from `package.json`; keep the local package at a stable path. Obsidian editing requires the user's separate Obsidian skill package; taskcli's generated structure is deterministic and does not launch an Agent itself.

SessionStart restores eligible Tasks and supplies task context. SessionEnd blocks active work and releases leases. Codex Interrupt does the same with reason `session interrupted` for an interrupted active main-thread turn; it preserves the Plan, fences the old token, and allows deletion once all relevant leases are released. Subsequent heartbeats do not reacquire released leases. Stop means a turn ended and only renews the lease. Tool hooks renew at tool boundaries; no hook daemon is spawned. A Codex/Claude operation or idle gap longer than 15 minutes can expire a lease. Pi/OMP extensions renew every minute while active; detected interruption and shutdown stop the timer and cancel in-flight renewal before releasing leases. Pi waits for agent_settled after an aborted result; OMP uses agent_end and excludes willContinue. New work restarts heartbeat without implicitly reclaiming a Task. The extensions inject current task facts before the agent runs, and expose a structured taskcli tool with session, executor, current lease token, and request idempotency key. A stale-token rejection requires inspecting and reacquiring work, never forcing a completion.

The shared SessionEnd hook, Codex Interrupt hook, and Claude PostToolUseFailure hook each request three seconds. Claude also has an overall session-exit budget, defaulting to 1.5 seconds; plugin hook timeouts do not raise it. Set `CLAUDE_CODE_SESSIONEND_HOOKS_TIMEOUT_MS=3000` when additional shutdown time is needed. If a busy database or interrupted process prevents shutdown cleanup, the next task operation reaps the expired lease. Other command hooks allow 30 seconds. Claude only releases on PostToolUseFailure when is_interrupt is the boolean true; ordinary tool errors do nothing. Claude has no general interrupt hook, and cancellation may emit no failure event. Use explicit cleanup after stopping work in that case. Pi/OMP aborts without an aborted assistant result also require shutdown or explicit cleanup. Disconnecting an idle Codex CLI from a persistent app-server does not guarantee Interrupt or immediate SessionEnd. After stopping the agent, `taskcli hook session-end --session SESSION_ID` explicitly releases its active leases; `taskcli hook interrupt --session SESSION_ID` records interruption instead. Update both taskcli and the installed plugin, then review the changed Codex hooks through `/hooks`; see the [lifecycle guide](../plugins/agent-task-manager/README.md#lifecycle-behavior).

Pi/OMP supplies an idempotency key for metadata mutations, including Job/Project deletion. Retrying the same deletion tool call returns the committed result without duplicate events. Within one Pi/OMP extension instance, the most recent 512 write requests retain their original injected lease token for idempotent retries, including after a successful write releases the lease or its response is lost. This token cache is not persisted across host restarts. Retrying beyond that window must preserve the original CLI request explicitly; do not assume a newly discovered lease will replay the old request.

Host session references remain unchanged so Agentix bindings can route notifications. Team context belongs to future Team tooling, keyed by `job_id`; `context --json`, cursor-based events, and optional `--delegated-by team:<id>` provide the integration boundary.

## Agentix integration

After initializing taskcli, add to Agentix's configuration:

```toml
[task_board]
config = "~/.config/taskcli/config.toml"
```

Restart Agentix after adding or changing `[task_board]`; taskcli being configured on its own does not enable the IM integration. With `[task_board]` enabled, Telegram registers `/dashboard` in its default command menu at startup, before any attachment. The top-level menu order is `/sessions`, `/dashboard`, `/cancel`, `/rmux`, `/help` (omit `/dashboard` when not configured). Contextual commands follow in alphabetical order. `/board` and `/jobs` appear in the chat menu only after attach; `/tasks` is not added to the menu.

`/dashboard` is the top-level IM dashboard. Each unarchived project has a button that opens its task board, grouped by status with full counts and clickable task entries. Project boards can be browsed without attaching a session.

After attach, `/board` and `/jobs` appear as contextual secondary menu commands. `/board` shows the current session's task board; `/jobs` lists all associated unarchived Jobs. Both find Jobs containing tasks with a matching lease or last recorded session. Sibling tasks show overall Job progress, and blocked/completed work remains visible after lease release until reassignment changes the last session or archival removes the Job from lists. Only unexpired leases are marked `Current`. Switching or detaching a session changes the scope and invalidates previous navigation buttons. Sessions without associated work get an empty-state message. These commands work independently of the agent's session-control capability, including read-only attachments.

Click a Job to read its authored Goal and Notes as Markdown, with buttons for its associated tasks and project board. Click a task from a board or Job to read its Task note body and current metadata. Every Task detail page includes a **Job** button for returning to its parent Job. YAML frontmatter, generated local task links and dependency graphs are excluded from the IM detail body. Telegram uses the existing MarkdownV2 conversion; Feishu uses its Markdown card element. Unavailable documents are reported while metadata and navigation remain accessible.

Project/Job/task lists have six entries per page with **Previous**/**Next** controls. Long detail bodies are also paged, preserving fenced code blocks across pages. For Job details, pagination advances both the authored content and associated task buttons; further pages remain available until both are exhausted. Browsing is read-only and does not update Plan hashes. Callback tokens use the existing conversation/owner, generation and binding-epoch checks.

`/projects` and `/sessionboard` are replaced by `/dashboard` and `/board`; `/board` and `/jobs` always use the current attachment. Legacy `/tasks [job-or-project]` and `/task <id>` remain direct shortcuts; the legacy task list is capped at 50 entries. An attached session can claim an unplanned Task or operate its own lease. Start is offered in PLANNING when Plan metadata and dependencies are ready; the service verifies the file at execution time. Done is offered only in EXECUTING. Block/Wait/Fail request a reason; `/cancel` clears pending input. Buttons use existing owner/conversation, generation, and binding-epoch checks plus the Task revision. IM does not create Jobs/Tasks or edit Plan bodies.

Agentix incrementally consumes SQLite events during its existing runtime tick. WAITING_USER, BLOCKED, FAILED, and Job completion notifications go only to the matching bound session's conversation. Events without a matching binding are skipped. Delivery retries on channel errors; a crash after send but before cursor persistence can duplicate a notification. CLI-only usage does not require Agentix to run.

## Validation

`make check` installs locked plugin dependencies, then runs Rust formatting, Clippy, workspace tests, and the Node built-in plugin tests. Install Node.js 24+ and npm. Direct Cargo invocations require `npm ci --ignore-scripts --prefix plugins/agent-task-manager` first. Normal tests use temporary databases/directories and local mock services, not live accounts.

| Boundary | Automated coverage |
| --- | --- |
| State and ownership | Seven Task states with IN_PROGRESS split into two phases against ten commands (80 cases), no partial writes on rejection, claim-before-Plan, Plan/start ownership, missing/blank Plans, lease renewal/recovery/handoff, stale tokens, dependency changes, archival |
| Processes and storage | Eight competing CLI processes with exactly one claim winner; four concurrent Jobs in both formats; start waits for Plan writes and rechecks lease expiry; v1 phase migration preserves leases/timestamps; kill after SQLite commit but before projection, then replay without duplicate events and repair files |
| Document projection | Dashboard Base/Markdown migration, collision preservation and retry, archive visibility and activity sorting; seven-state Mermaid graphs with cross-Job prerequisites and renamed links; TaskNotes Bases and per-task notes in both formats, exact folder/project scope, frontmatter state projection and repair without SQLite writes, legacy Plan-path migration, editable Notes under concurrent writes, safe marker/symlink failures, and YAML-frontmatter Plan bodies |
| Host plugin | Actual Pi/OMP TypeScript entrypoints and lifecycle hooks invoke the compiled CLI; structured tool schema, plans, leases, both link formats, retry identity after lease release/lost responses and deletion, ordinary Claude failures and automatic Pi/OMP continuations that retain ownership, errors, aborts, identity fencing, and periodic heartbeat behavior |
| IM orchestration | Session/revision/owner scoping, Wait/Fail reasons, cancellation, Job completion, notification paging, route isolation, retry after channel failure, durable delivery cursor after Engine reconstruction |
| Channel adapters | Actual Telegram HTTP and Feishu HTTP/WebSocket adapters pass task callbacks and reason messages through the Engine; tests verify SQLite state, projected Markdown, and notifications at local mock APIs |

The plugin tests use a minimal host API harness, not installed Pi/OMP loaders or model-generated tool calls. Codex/Claude hook tests execute their manifest commands with representative payloads, not live host sessions. CI runs the normal suite on Linux/macOS and task core/CLI/plugin checks on Windows; Unix-only symlink cases are excluded on Windows.

For actual Obsidian rendering, enable the separate ignored desktop test explicitly. Open a test vault with TaskNotes and Bases enabled, bring its window to the foreground, enable the Obsidian CLI, and ensure the chosen parent directory already exists:

```sh
TASKCLI_OBSIDIAN_VAULT="Test vault" TASKCLI_OBSIDIAN_PARENT="Tests" \
  cargo test -p taskcli --test obsidian_smoke -- --ignored --nocapture
```

`OBSIDIAN_BIN` can select a specific CLI executable. For each format, the test creates an isolated `taskcli-smoke-*` directory under that parent (default `00-Inbox/agent`) and a temporary tab, then restores the previous tab and deletes only its own generated files. It checks Dashboard columns, dates, archive/unarchive filtering and link targets through native navigation, plus rendered TaskNotes Kanban columns and cards, task note recognition, and note links. The visibility prerequisite prevents hidden-window rendering from being mistaken for an empty board. It does not install or enable plugins. A force-killed test process can leave its temporary directory/tab behind; do not run it concurrently with manual edits in that directory.

The [integration coverage map](integration-coverage.md) links each behavior to its executable tests. These tests do not establish complete branch coverage or live-system acceptance. Real IM credentials/permissions, host installer and loader compatibility, model-directed tool selection, desktop themes/plugins, and multi-machine/network-filesystem behavior require separate checks. The supported concurrency target remains multiple local processes on one computer.
