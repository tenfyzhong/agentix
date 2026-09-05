# Task board and standalone CLI

`agentix-task` supplies SQLite coordination and document projection; `taskcli` is its independent command-line interface. Agentix optionally uses the same library and database for IM control. Agent Team membership, scheduling, and shared context are external concerns, associated through stable Job IDs and optional `delegated_by` metadata.

## Model and concurrency

A Project identifies a Git repository or stable directory. Git worktrees share their repository's common directory and reuse one Project. Jobs represent independently acceptable requirements, while Tasks are executable steps. New requirements after delivery create new Jobs; completed and cancelled Jobs can be manually archived by UTC year and month. There is no milestone layer or Job-level exclusive lock.

Tasks use `TODO`, `IN_PROGRESS`, `BLOCKED`, `WAITING_USER`, `DONE`, `FAILED`, and `CANCELLED`. TODO can be claimed, blocked, put into WAITING_USER, or cancelled. IN_PROGRESS can finish, fail, block, wait, or cancel. BLOCKED and WAITING_USER can switch between each other, resume through claim, fail, or cancel. FAILED requires `retry`; DONE/CANCELLED require `reopen`. Reopening a prerequisite after a downstream Task started is rejected.

Jobs aggregate to COMPLETED once at least one non-cancelled Task exists and all such Tasks are DONE. Cancelling every Task does not count as delivering the requirement. Completed Jobs reject additional Tasks. Reopen corrects a Task's result and returns its Job to ACTIVE; new scope belongs in a new Job. Active Jobs cannot be archived. Job cancellation requires its active Task leases to be released first.

Each Task has at most one effective lease, and each executor/session pair has at most one Task. Claim atomically checks the Plan, dependencies, state, and ownership. Different Tasks and Jobs can execute concurrently. Dependencies must remain inside one Project and cannot form cycles or change after execution starts. Leases coordinate task facts, not working directories: code Tasks need their own branches/worktrees and dependencies for shared resources.

SQLite uses WAL, a ten-second busy timeout, short immediate write transactions, foreign keys, revision checks, and optional idempotency keys. Entity documents are stored as typed JSON rows with relational generated columns and indexes; dependency edges and events have their own tables. Mutations load the local task graph inside a write transaction to validate invariants and persist only changed entities. This first version targets local repositories and modest task graphs on one computer, not shared database files on network filesystems.

## Configuration

```sh
taskcli init --format markdown --root /existing/documents --directory "Agent Tasks"
taskcli init --format obsidian --root /existing/vault --directory "Agent Tasks"
```

Run only the applicable initialization. Default config: `~/.config/taskcli/config.toml`; override with `--config` or `TASKCLI_CONFIG`. Initialization refuses to overwrite an existing config. `--database` selects the independent task database. The config has this shape:

```toml
schema_version = 1

[storage]
path = "~/.local/share/taskcli/tasks.sqlite3"

[documents]
format = "markdown"
root = "/existing/documents"
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
taskcli plan create task_STORAGE --file /work/storage-plan.md
taskcli task claim task_STORAGE --executor agent:member --session HOST_SESSION --json
```

IDs use UUIDv7 with `prj_`, `job_`, `task_`, and `plan_` prefixes; full IDs or unambiguous prefixes are accepted. `project show` also accepts an unambiguous project name. Outside Git, pass `--project` explicitly. `task list --ready` discovers TODO work with completed dependencies; create its Plan before claim. Populate all initially required Tasks before finishing the first one so Job completion reflects the agreed scope.

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

A lease lasts 15 minutes. Renew at least once a minute during execution. Terminal, blocked, waiting, and release operations remove the lease. Abnormal exit hooks and expired leases create a system BLOCKED reason. Expiry is checked on CLI/library operations and Agentix refresh, without a standalone background daemon. Resuming the same session reacquires only system-blocked Tasks that have not been taken over; it issues new tokens. Manual blocks stay blocked. A missing Plan prevents automatic resume.

Use `--expect-revision N` to protect an update based on an earlier read. Use `--idempotency-key KEY` to retry identical requests without duplicate entities/events; reuse with different arguments fails. Local CLI access is not a Team authorization boundary. Lease tokens fence stale executions, while operating-system access controls protect the local files.

`job update`, `task update`, `task depend/undepend`, `plan revise`, `job cancel`, and `job archive/unarchive` provide the remaining mutations. Consult `--help` for each command.

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
Dashboard.md
Projects/<project-key>/
  Board.md
  Sync Status.md
  Jobs/Active/<job-id>.md
  Jobs/Archive/YYYY/MM/<job-id>.md
  Plans/<task-id>/v001.md
```

Boards contain seven static state columns. They have no Kanban metadata or task checkboxes, and do not require community plugins. Obsidian links are vault-relative wikilinks (escaped correctly inside tables); plain Markdown links are source-relative URLs with encoded path segments. Task anchors use Obsidian block references or Markdown HTML anchors.

Obsidian does not decode HTML entities in wikilink aliases. Titles containing reserved characters such as `|`, brackets, or HTML delimiters therefore appear as a stable `Open` wikilink followed by the safely escaped title; ordinary titles remain the link label. This preserves readable punctuation without injecting extra links or breaking the table.

Generated regions are logically read-only, not filesystem-protected. Manual edits to status, title, dependencies, or ordering are overwritten by the next projection and never imported. There is no `watch` command. Goal/Notes markers preserve their editable bodies. Explicit `job update --goal` replaces a manually edited Goal; Notes remain untouched. Missing/duplicated editable markers fail synchronization instead of dropping content.

Plan files contain authoritative Markdown bodies. `plan revise` creates a new version and preserves previous files. Manual edits refresh hashes during `sync`, `plan show`, and claim; incidental file history can be managed by Git. Agents editing Obsidian bodies must load the available Obsidian skill and use `[[wikilinks]]`; other directories use standard Markdown.

Database commits happen before generated-document updates. A filesystem failure returns success with `projection_pending` so callers do not recreate committed work. `taskcli sync` repairs the projection. Output file locks serialize independent CLI processes; temporary-file replacement protects each document. Archival writes the destination and updates managed links before removing the previous generated file. Back up the database and document tree together; editable bodies are not recoverable from SQLite alone.

```sh
taskcli doctor --json  # healthy, missing_plans, database/projection sequence
taskcli sync
```

## Host plugin

The shared package is `plugins/agent-task-manager`, also included in release archives and Homebrew's `share/agentix/plugins`. It has Codex/Claude manifests, a shared Skill, command hooks, and Pi/OMP TypeScript entrypoints. Node.js 22 or newer is required for command hooks. Put `taskcli` on PATH or set `TASKCLI_BIN`; set `TASKCLI_CONFIG` when using a non-default config.

For local Codex plugin development, use the host's plugin installation flow with this package directory. The manifest exposes `skills/`, and Codex discovers `hooks/hooks.json`. Claude can load it with `claude --plugin-dir /absolute/path/to/agent-task-manager`. These hosts share the default hook file without duplicate manifest declarations. Review/enable hooks in the host as required; Codex requires reviewing and trusting plugin hooks through `/hooks`. The command resolves plugin-root environment variables inside Node rather than using shell-specific expansion.

The package explicitly includes its manifests, hooks, extensions, runtime, skills, and activation guide in npm distributions. Pi and OMP each select their own entrypoint through `package.json`; installing the complete package does not require copying hooks into project settings. See the [plugin activation and lifecycle guide](../plugins/agent-task-manager/README.md).

For Pi/OMP, install the local package with their package/plugin manager or load `extensions/pi.ts` / `extensions/omp.ts`. A manually copied package needs its dependency installed:

```sh
npm ci --ignore-scripts --prefix /absolute/path/to/agent-task-manager
pi -e /absolute/path/to/agent-task-manager/extensions/pi.ts
omp -e /absolute/path/to/agent-task-manager/extensions/omp.ts
```

Installing the package also exposes its shared Skill. When loading only an extension entrypoint, configure the sibling `skills/` directory in the host's skill paths as well. Obsidian editing requires the user's separate Obsidian skill package; taskcli's generated structure is deterministic and does not launch an Agent itself.

SessionStart restores eligible Tasks and supplies task context. SessionEnd blocks active work. Stop means a turn ended and only renews the lease. Tool hooks renew at tool boundaries; no hook daemon is spawned. A Codex/Claude operation or idle gap longer than 15 minutes can expire a lease. Pi/OMP extensions renew every minute while the session is open, inject current task facts before the agent runs, and expose a structured taskcli tool with session, executor, current lease token, and request idempotency key. A stale-token rejection requires inspecting and reacquiring work, never forcing a completion.

The shared SessionEnd hook allows three seconds, respecting Codex's shutdown timeout limit. If a busy database or interrupted process prevents shutdown cleanup, the next task operation reaps the expired lease. Other command hooks allow 30 seconds.

Within one Pi/OMP extension instance, the most recent 512 write requests retain their original injected lease token for idempotent retries, including after a successful write releases the lease or its response is lost. This token cache is not persisted across host restarts. Retrying beyond that window must preserve the original CLI request explicitly; do not assume a newly discovered lease will replay the old request.

Host session references remain unchanged so Agentix bindings can route notifications. Team context belongs to future Team tooling, keyed by `job_id`; `context --json`, cursor-based events, and optional `--delegated-by team:<id>` provide the integration boundary.

## Agentix integration

After initializing taskcli, add to Agentix's configuration:

```toml
[task_board]
config = "~/.config/taskcli/config.toml"
```

`/jobs [project]`, `/tasks [job-or-project]`, and `/task <id>` browse tasks. Lists are capped at 50 entries; use a project/job filter or CLI for larger histories. An attached session can claim eligible Tasks or operate its own lease. Block/Wait/Fail request a reason; `/cancel` clears pending input. Buttons use existing owner/conversation, generation, and binding-epoch checks plus the Task revision. IM does not create Jobs/Tasks or edit Plan bodies.

Agentix incrementally consumes SQLite events during its existing runtime tick. WAITING_USER, BLOCKED, FAILED, and Job completion notifications go only to the matching bound session's conversation. Events without a matching binding are skipped. Delivery retries on channel errors; a crash after send but before cursor persistence can duplicate a notification. CLI-only usage does not require Agentix to run.

## Validation

`make check` installs locked plugin dependencies, then runs Rust formatting, Clippy, workspace tests, and the Node built-in plugin tests. Install Node.js 24+ and npm. Direct Cargo invocations require `npm ci --ignore-scripts --prefix plugins/agent-task-manager` first. Normal tests use temporary databases/directories and local mock services, not live accounts.

| Boundary | Automated coverage |
| --- | --- |
| State and ownership | All seven Task states against nine commands (63 cases), no partial writes on rejection, same-session claim limits, lease renewal/handoff, stale tokens, ownership/revision checks, dependency changes, archival |
| Processes and storage | Eight competing CLI processes with exactly one claim winner; four concurrent Jobs in both formats; kill after SQLite commit but before projection, then replay without duplicate events and repair files |
| Document projection | Editable Notes survive concurrent writes; malformed markers and symlink escapes fail safely; unsupported schema versions are not downgraded; CLI rejects invalid inputs without changing state/configuration; YAML-frontmatter Plan bodies round-trip |
| Host plugin | Actual Pi/OMP TypeScript entrypoints and lifecycle hooks invoke the compiled CLI; structured tool schema, plans, leases, both link formats, retry identity after lease release/lost responses, errors, aborts, identity fencing, and periodic heartbeat behavior |
| IM orchestration | Session/revision/owner scoping, Wait/Fail reasons, cancellation, Job completion, notification paging, route isolation, retry after channel failure, durable delivery cursor after Engine reconstruction |
| Channel adapters | Actual Telegram HTTP and Feishu HTTP/WebSocket adapters pass task callbacks and reason messages through the Engine; tests verify SQLite state, projected Markdown, and notifications at local mock APIs |

The plugin tests use a minimal host API harness, not installed Pi/OMP loaders or model-generated tool calls. Codex/Claude hook tests execute their manifest commands with representative payloads, not live host sessions. CI runs the normal suite on Linux/macOS and task core/CLI/plugin checks on Windows; Unix-only symlink cases are excluded on Windows.

For actual Obsidian rendering, enable the separate ignored desktop test explicitly. Open a test vault, enable the Obsidian CLI, and ensure the chosen parent directory already exists:

```sh
TASKCLI_OBSIDIAN_VAULT="Test vault" TASKCLI_OBSIDIAN_PARENT="Tests" \
  cargo test -p taskcli --test obsidian_smoke -- --ignored --nocapture
```

`OBSIDIAN_BIN` can select a specific CLI executable. The test creates an isolated `taskcli-smoke-*` directory under that parent (default `00-Inbox/agent`) and a temporary tab, then restores the previous tab and deletes only its own generated files. It verifies seven rendered columns, no interactive checkboxes, Unicode/punctuation labels, Plan navigation, and Task block anchors. A force-killed test process can leave its temporary directory/tab behind; do not run it concurrently with manual edits in that directory.

These tests do not establish complete branch coverage or live-system acceptance. Real IM credentials/permissions, host installer and loader compatibility, model-directed tool selection, desktop themes/plugins, and multi-machine/network-filesystem behavior require separate checks. The supported concurrency target remains multiple local processes on one computer.
