# Task board and standalone CLI

`agentix-task` supplies SQLite coordination and document projection; `taskix` is its independent command-line interface. Agentix optionally uses the same library and database for IM control. Agent Team membership, scheduling, and shared context are external concerns, associated through stable Job IDs and optional `delegated_by` metadata.

## Model and concurrency

A Project identifies a Git repository or stable directory. Git worktrees share their repository's common directory and reuse one Project. Jobs represent independently acceptable requirements, while Tasks are executable steps. New requirements after delivery create new Jobs; completed and cancelled Jobs can be manually archived into `Jobs/Archived/`. Unarchived Jobs, including completed ones, remain directly in `Jobs/`. There is no milestone layer or Job-level exclusive lock.

Tasks use `TODO`, `IN_PROGRESS`, `BLOCKED`, `WAITING_USER`, `DONE`, `FAILED`, and `CANCELLED`. IN_PROGRESS has two phases: claim enters `PLANNING`; explicit start enters `EXECUTING`. Only EXECUTING can finish with done. Both phases can fail, block, wait, release, or cancel. TODO can be claimed, blocked, put into WAITING_USER, or cancelled. BLOCKED and WAITING_USER can switch between each other, return to PLANNING through claim, fail, or cancel. FAILED requires `retry`; DONE/CANCELLED require `reopen`. Outside IN_PROGRESS, `phase` is null. Reopening a prerequisite after a downstream Task started executing is rejected; planning alone does not freeze dependencies.

Jobs with the default `review_policy: required` enter PENDING_REVIEW once at least one non-cancelled Task exists and all such Tasks are DONE; `review_policy: none` Jobs complete directly at that point. `job approve` marks verification passed and completes the Job; `job reject --reason` returns it to ACTIVE without changing Task states. `job submit` explicitly resubmits a ready rejected Job. Metadata edits and sync do not resubmit it. Cancelling every Task does not count as delivering the requirement. Completed Jobs reject additional Tasks. Reopen corrects a Task's result and returns its Job to ACTIVE. Related supplements to pending review use `job followup` and new Tasks in the same Job; independent requirements and requests after completion belong in a new Job. ACTIVE and PENDING_REVIEW Jobs cannot be archived. Job cancellation requires its active Task leases to be released first.

Each Task has at most one effective lease, and each executor/session pair has at most one Task. Claim atomically checks state and ownership, reserving the Task before an agent drafts its Plan; it does not require a Plan or completed dependencies. Plan creation/revision requires the active session and lease token. Start checks ownership, a nonblank current Plan file, and DONE dependencies before switching to EXECUTING under the same lease. Plan writes and start validation share an output lock; state/lease checks run again in the database transaction. `started_at` records the first execution start, not claim time. Different Tasks and Jobs can plan or execute concurrently. Dependencies must remain inside one Project and cannot form cycles or change after execution starts. Leases coordinate task facts, not working directories: code Tasks need their own branches/worktrees and dependencies for shared resources.

SQLite uses WAL, a ten-second busy timeout, short immediate write transactions, foreign keys, revision checks, and optional idempotency keys. Entity documents are stored as typed JSON rows with relational generated columns and indexes; dependency edges and events have their own tables. Mutations load the local task graph inside a write transaction to validate invariants and persist only changed entities. This first version targets local repositories and modest task graphs on one computer, not shared database files on network filesystems.

Database schema v7 moves Plan files into Task notes, retains durable deletion cleanup and document sequence counters, and migrates earlier databases to flat `Jobs/` and `Jobs/Archived/` directories, dated and numbered Job and Task Plan filenames, readable project names, project archival, lifecycle timestamps, and one current Plan per Task. Existing Jobs and Tasks receive daily sequence numbers in creation order (ID breaks timestamp ties), independently per project and entity type. The migration preserves IDs, leases, editable Goal/Notes, and the latest Plan body. Old Plan files are removed only after the replacement documents are written; empty old directories are removed too. Back up SQLite and documents together before upgrading all writers. Binaries predating that migration reject v7. Configuration and JSON envelope `schema_version` remain 1; database `PRAGMA user_version` is separate.

Schema v11 records each Job’s most recent `pending_review_at` and recovers existing review timestamps from the event log (falling back to the last update for legacy pending Jobs without an event). It also preserves IM source-message associations, including those recoverable from earlier submission receipts, and visible Job conversations. Upgrade all writers together before opening the database.

Schema v10 aligns Inbox states with Jobs, migrating IN_PROGRESS/DONE to ACTIVE/COMPLETED and restoring PENDING_REVIEW from the linked Job. Upgrade all writers together. Sync publishes the five distinct checkbox symbols; old state names remain accepted CLI aliases.

Schema v8 adds Project Inbox entries, their exclusive leases, deletion tombstones, and publication state without changing existing Job/Task identities or Plans. Upgrade all taskix and Agentix writers together; older binaries reject v8. Existing Projects receive `Inbox.md` on synchronization.

Schema v9 adds PENDING_REVIEW and optional `review_reason`; historical COMPLETED Jobs stay completed. Upgrade all database writers together. See the [Task and Job state machines](task-state-machines.md).

## Project inbox

Each Project has `Projects/<project-name>/Inbox.md`, linked from its Board. Humans can append requirements in that document or through IM. Inside the generated Inbox region, one top-level checkbox is one requirement; indent its details underneath:

```markdown
- [ ] Add CSV export
  Preserve filters and column order.
  - [ ] Include a header row
  - [ ] Cover Unicode values
- [ ] Add an export history page
```

Nested checklists and fenced examples do not become separate entries. Synchronization adds stable `inbox_` IDs; keep these ID comments and the document's start/end markers intact. Identical titles are allowed. Direct document text edits are imported only while an entry is `TODO` and has no linked Job; releasing an entry does not make its text editable again. Reorder whole entries, including their IDs and indented bodies. New IM submissions go at the end. Generated items have no extra blank separator; paragraph breaks inside a requirement are preserved. Editing a registered IM `/inbox` message updates that same entry and its document, preserving status, order, lease, and linked Job. Telegram uses edit events; Feishu checks registered source messages every 30 seconds while Agentix runs. Matching sender and conversation identity are required; duplicate and older updates are ignored. Associations survive restart and attachment changes. Removing the `/inbox` command from a message does not execute a different command or delete the entry. Archived Projects and withdrawn entries reject edits. Failed document publication is recoverable on sync. Reserved `<!-- taskix:` control markers cannot be submitted as content.

| Inbox status | Meaning |
| --- | --- |
| `TODO` | Queued, or unfinished work released for recovery |
| `ACTIVE` | Its Job is active, with an optional agent lease; rendered as `- [/]` |
| `PENDING_REVIEW` | Its Job awaits verification, without an Inbox lease; rendered as `- [r]` |
| `COMPLETED` | Its formal Job passed verification, or an unlinked item was completed manually; rendered as `- [x]` |
| `CANCELLED` | Withdrawn by the human or its formal Job cancelled; rendered as `- [-]` |

Managed metadata stays on the entry's checkbox line: the ID and status are HTML comments, followed by a visible Job link and current executor when present. For example:

```markdown
- [ ] Check feature completeness <!-- taskix:entry:inbox_01a07760d6a673f2a863e0f105eb9783 --> <!-- taskix:entry-state TODO revision=1 -->
```

Synchronization upgrades older receipts to this inline format with a revision, preserving entry IDs and authored details. Legacy `[p]` review markers remain readable; synchronization and status repair write the canonical `[r]` marker without changing the entry state or revision. The connected plugin maps `[ ]`, `[/]`, `[r]`, `[x]`, `[-]` to TODO, ACTIVE, PENDING_REVIEW, COMPLETED, CANCELLED through `inbox set-status`. Manual status changes never create Jobs or claim agent leases. Unlinked items can be marked ACTIVE or PENDING_REVIEW and completed directly after reopening if cancelled. Activating a linked item resumes its existing Job. Linked review submission requires ready Tasks; checking a linked item approves its pending verification. Reopening preserves the Job ID and Task history. Returning to TODO requires active leases to be released first. Unsupported edits restore the checkbox and show a notification. See [Inbox checkbox edits](../plugins/taskix-manager/obsidian/README.md#inbox-checkbox-edits).

Set an unfinished item to `- [-]` to cancel, or delete it to withdraw it. Cancellation revokes Inbox and associated Task leases, cancels unfinished Tasks and the active Job, and preserves completed/failed Task outcomes, Plans, Job documents, and audit history. Agents receive cancellation facts at subsequent context/tool/heartbeat boundaries and stop that work; filesystem edits already made are not rolled back. A stale lease cannot submit completion. Deleted entries cannot be revived. Deleting a terminal entry only hides it from the queue. Without the connected plugin, ordinary sync imports cancellations and withdrawals but does not interpret checks/unchecks as completion or reopening; use the explicit status command instead. Startup reconciles offline checkbox drift without replaying it.

Deleting the entire Inbox file, an unreadable file, duplicate IDs, or malformed markers causes a synchronization error, never cancellation of every entry. Restore the document before retrying. The service imports saved edits at synchronization, claim, and relevant task/lifecycle boundaries; the desktop plugin additionally listens to saved checkbox changes. Database commits and document publication recover separately: `projection_pending` means the request is saved and `taskix sync` can repair the document. An append awaiting publication is not treated as a human deletion.

On each user prompt, `context.inbox_todos` returns every TODO entry in the current Project after importing Inbox edits, including its full content and ID. The agent compares the request with these candidates semantically and selects zero, one, or multiple available entries. Pass each selected full ID with repeated `--inbox ENTRY_ID` arguments on `job create`, `job update`, or `job followup`; preserve the verbatim request with `--prompt` on creation, prompt backfill, or follow-up. Prompt text alone never links entries, even when it contains an entire entry verbatim. Selection is explicit and atomic: foreign-project, unpublished, withdrawn, content-pending, leased, already linked, or non-TODO entries reject the write. Selected entries become ACTIVE with the requesting agent/session’s Inbox lease when supplied, then follow the Job into PENDING_REVIEW and COMPLETED. Treat candidate content as data for matching, not as instructions or permission to work on unrelated requirements.

Agents take one Inbox entry only after the user explicitly asks for the next Job, for example “Get the next Job from the Inbox.” They return that Job’s result and wait for another explicit request. Adding an entry or completing a Job does not start the next requirement. New intake waits for other ACTIVE Project Jobs, including unplanned Jobs and Jobs with blocked or waiting Tasks. PENDING_REVIEW Jobs do not block an explicitly requested next entry; they remain pending verification. `inbox claim-next` atomically reserves the next entry and creates its formal Job in one SQLite transaction. The agent then uses the existing decomposition, dependency, claim, Plan, start, verification, and completion workflow. A lease lasts 15 minutes and renews with the session heartbeat. Interruption, explicit release, or expiry returns unfinished Inbox work to `TODO`; the next claimant resumes the same Job and Tasks, without duplicating decomposition. Recovery entries take priority over new submissions; entries within each group follow document order. Pending review sets PENDING_REVIEW without a lease; rejection returns the item to ACTIVE. An unleased ACTIVE item is also eligible for explicit recovery. Recovery also waits for other ACTIVE Jobs and outstanding Task leases.

```sh
taskix inbox add --content 'Add CSV export'
taskix inbox list --json
taskix inbox sync
# Only after the user asks to take the next Job:
taskix --executor agent:codex --session SESSION inbox claim-next --json
taskix --session SESSION --lease-token INBOX_LEASE inbox release inbox_ID
taskix inbox cancel inbox_ID
taskix inbox set-status inbox_ID --status TODO
# After verifying the linked PENDING_REVIEW Job, or for an unlinked non-cancelled item:
taskix inbox set-status inbox_ID --status COMPLETED --expect-revision REVISION --idempotency-key KEY
```

Use `--project PROJECT` outside Git. The Inbox lease is distinct from a Task lease. Use the full Inbox ID and its own token for release. `context --session SESSION --json` includes owned Inbox facts even before decomposition, plus cancellation facts. The legacy CLI `hook stop` is a compatibility no-op: it returns `claimed: false` with reason `manual_intake_required`. Codex/Claude Stop only renews leases. Pi/OMP completion and idle callbacks handle lifecycle state without claiming work or requesting Inbox follow-ups.

After finishing the current request, running commands, verification, and result preparation, return the result and wait for human input. Use `inbox list` for inspection; `claim-next` reserves work and requires an explicit request to take the next Job. If no entry is eligible, report the reason and wait for further input.

### Submitting through IM

With task boards configured, attach a session and use the secondary commands `/inboxes` and `/inbox <content>`. `/inboxes` lists the current Project's human entries, including all five statuses, in document order with six per page. Select an entry to read its Markdown and navigate to its formal Job. Deleted entries are excluded. `/jobs` continues to list the session's associated formal Jobs.

One `/inbox` message appends one `TODO` entry, preserving newlines, internal spaces, and Markdown. Empty content displays usage. The reply includes the Project, entry ID, status, and navigation buttons. Submission uses the task service directly and also works on read-only agent attachments; it does not send a prompt or start an agent turn. Channel, conversation, and inbound message identity form a durable idempotency key, so delivery retries and restarts do not duplicate submissions.

Project resolution uses the attached session's directory and Git common directory, including worktrees. Without an available directory, a unique recorded Project association is required. Missing or ambiguous associations must be resolved by attaching the correct session; IM does not register or guess Projects. Navigation remains scoped to the owner, conversation, and attachment epoch, so switching sessions invalidates old buttons.

Directory resolution reads only Project records and selects the closest registered ancestor for non-Git directories. Historical resolution uses the existing Task, Job, and Inbox session indexes to find up to two distinct Project IDs; it loads only the matching Project when the association is unique. Neither path loads a full database snapshot, and calls without a usable directory or session return without querying the database.

CLI Job lists apply status, archive visibility, creation-date bounds, and archive-month bounds in SQL before loading Job bodies. Date bounds remain UTC, the creation end date includes the whole day, and results retain insertion order.

IM Inbox views read only the requested Project and entry, re-reading the entry after document synchronization. Source edits resolve the entry through a source index. The periodic source refresh reads only IDs, source references, and source versions for published, non-deleted entries in unarchived Projects; it does not load Job, Task, Plan, or Inbox bodies.

The legacy IM `/tasks` list applies its Project or Job filter and 50-row limit in SQL. Task buttons read only the target lease before applying the existing revision and ownership checks. Task detail snapshots fetch dependency status by ID, so a completed dependency outside the displayed Task still enables Start without loading that dependency’s body.

After writing document files, publication commits Plan hashes, Job goal metadata, registered paths, and generation-checked pending acknowledgements in one transaction. Plan updates remain conditional on the published version, and failed acknowledgements roll back all publication metadata so retries can recover. Job goal metadata and document receipt rows use batched queries.

Full document publication prepares borrowed Project, Job, Task, and Plan lookups once, groups Tasks by Job in document order, and indexes registered paths before checking destination ownership. Obsidian metadata snapshots strip authored bodies and Inbox credentials in SQL, omit Plans and task leases, and read the three note types in one transaction. They share property formatting with single-note reads instead of rendering and reparsing Markdown.

CLI Project and Job detail reads load only the requested entity. Project lists load Project records, and Job lists restrict reads to the selected Project when supplied. Task lists apply Job, Project, status, and readiness filters in SQLite; readiness checks dependency status by ID, including dependencies in other Jobs. These commands preserve insertion order and the existing lease-expiry maintenance behavior.

`context` loads the selected assignment, current Plan, owned Inbox, and session cancellation notifications. Previous-Job selection ranks session activity before loading the winning Job body and its Task IDs, preserving same-second follow-up ordering and ignoring late assistant replies. Session and follow-up indexes keep unrelated historical Jobs out of this lookup; the indexes are installed automatically when opening the database.

## Shell completions

`taskix completions bash`, `taskix completions zsh`, and `taskix completions fish` print shell scripts directly, including when `--json` is present. Generation skips configuration loading and does not open or mutate the task database, so it works before `taskix init`.

Source checkouts and `taskix-*` release archives include `completions/taskix.bash`, `completions/_taskix`, and `completions/taskix.fish`. Follow the [shell installation instructions](../README.md#shell-completions). Contributors regenerate all Agentix and taskix scripts with `make completions`; tests compare the generated output with these files and exercise nested commands, options and file paths.

## Configuration

Task boards require an existing Obsidian vault with TaskNotes and Bases enabled for rendered boards, task queries, and wikilink navigation. See [Obsidian plugin setup](#obsidian-plugin-setup).

```sh
taskix init --root /existing/vault --directory "Agent Tasks"
```

Default config: `~/.config/taskix/config.toml`; override with `--config` or `TASKIX_CONFIG`. Initialization refuses to overwrite an existing config. `--database` selects the independent task database. The config has this shape:

```toml
schema_version = 1

[storage]
path = "~/.local/share/taskix/tasks.sqlite3"

[documents]
root = "/existing/vault"
directory = "Agent Tasks"
```

The root must be an existing absolute Obsidian vault directory containing `.obsidian`. The output subdirectory is relative, has no traversal components, and may be `.`. The database must be outside the output directory and separate from Agentix runtime storage. Task databases carry a SQLite application identifier; taskix rejects unrelated databases, and Agentix refuses to add its runtime tables to task databases. Symlinks cannot make document paths escape their configured root. Keep one output configuration per database; configuration relocation is an explicit migration, not automatic synchronization.

## Workflow

```sh
taskix project register                       # Run in the Git worktree
taskix project register --root /work/docs --name Docs
taskix job create --project prj_ID --title "New requirement" --goal "Acceptance checks" --prompt "Original user request" --executor agent:HOST --session HOST_SESSION
taskix task add --job job_ID --title "Implement and verify the storage layer" --executor agent:HOST --session HOST_SESSION
taskix task add --job job_ID --title "Integrate the client" --executor agent:HOST --session HOST_SESSION
taskix task depend task_CLIENT task_STORAGE
taskix task claim task_STORAGE --executor agent:HOST:member --session HOST_SESSION --json
# After claim succeeds, draft the Plan and publish it with the returned token:
taskix plan create task_STORAGE --file /work/storage-plan.md --session HOST_SESSION --lease-token lease_TOKEN
taskix task start task_STORAGE --session HOST_SESSION --lease-token lease_TOKEN
# Execute the Plan, verify acceptance, then call done with the same token.
```

IDs use UUIDv7 with `prj_`, `job_`, `task_`, and `plan_` prefixes; full IDs or unambiguous prefixes are accepted. `project show` also accepts an unambiguous project name. Outside Git, pass `--project` explicitly. `task list --ready` discovers TODO work with completed dependencies; `task list --status TODO` also includes work that can be planned before dependencies finish. Claim before drafting either kind of Task. Register all known initial Tasks and dependency edges before implementation, verify their generated notes, and prepare each detailed Plan when taking up its Task so Job completion reflects the agreed scope.

`task show ID` reads the Task and its lease by primary key in one transaction. Unique prefixes use an indexed range limited to two candidate IDs. It does not load other entities or perform global expiry maintenance; it reports stored state, including the stored lease expiry. Lifecycle commands continue enforcing lease expiry. The Obsidian `show ID` query follows the same read-only principle and accepts exact Task, Job, or Inbox IDs.

Individual Plan, Task Markdown, and Job Markdown reads also avoid full database snapshots: they load only the selected Task and current Plan, the Task and its parent Project, or the selected Job, respectively. Job-filtered event pages resolve the Job ID through its index and seek using the existing `(job_id, sequence)` index, so unrelated entity bodies and other Jobs' event histories are not loaded. Unique prefixes, cursor ordering, document path validation, and Plan hash refresh behavior are preserved.

Claim returns the Task and a `lease` containing its token. Subsequent writes to a leased Task must include the current session and token:

```sh
taskix task heartbeat task_ID --session HOST_SESSION --lease-token lease_TOKEN
taskix task done task_ID --session HOST_SESSION --lease-token lease_TOKEN
taskix task block task_ID --reason "Upstream unavailable" --session HOST_SESSION --lease-token lease_TOKEN
taskix task wait task_ID --reason "Need a decision" --session HOST_SESSION --lease-token lease_TOKEN
taskix task fail task_ID --reason "Acceptance test failed" --session HOST_SESSION --lease-token lease_TOKEN
taskix task release task_ID --reason "Handing off" --session HOST_SESSION --lease-token lease_TOKEN
taskix task retry task_ID
taskix task reopen task_ID
```

A lease lasts 15 minutes. Renew at least once a minute during planning and execution. Terminal, blocked, waiting, and release operations remove the lease and clear the phase. Abnormal exit hooks and expired leases create a system BLOCKED reason. Expiry is checked on CLI/library operations and Agentix refresh, without a standalone background daemon. Resuming the same session reacquires only system-blocked Tasks that have not been taken over; it issues new tokens and returns to PLANNING. Manual blocks stay blocked. Missing Plans do not prevent planning recovery; repair/review the Plan and explicitly call start before continuing execution. Hooks never automatically start or finish work.

Use `--expect-revision N` to protect an update based on an earlier read. Use `--idempotency-key KEY` to retry identical requests without duplicate entities/events; reuse with different arguments fails. Local CLI access is not a Team authorization boundary. Lease tokens fence stale executions, while operating-system access controls protect the local files.

`job update`, `task update`, `task depend/undepend`, `plan revise`, `job followup`, `job submit/approve/reject`, `job cancel`, `job archive/unarchive`, `job delete`, and `project delete` provide the remaining mutations. Job deletion cleans up registered document paths and unregistered destinations only when their generated entity identity matches. An unrelated note that blocked creation or renaming is preserved. Project deletion still removes the entire project directory, including attachments. Consult `--help` for each command.

```sh
taskix job list --active
taskix job list --pending-review
taskix job list --completed
taskix job list --archived --period 2026-09
taskix job list --created-from 2026-07-01 --created-to 2026-09-30
taskix event list --job job_ID --after 0 --limit 100 --json
taskix context --session HOST_SESSION --json
```

Timestamp fields are Unix seconds in UTC; creation date filters include both specified calendar dates. JSON responses carry `schema_version: 1`, `ok`, and either `result` or `error`. Mutation responses also include `sequence` and `projection_pending`. Exit status is 0 on success, 1 on business/runtime failure, and 2 on argument errors. Event listing returns ordered events and `next_cursor`; the limit is 1–1000. Event payloads contain no Plan body.

## Document projections

```text
Dashboard.base
Projects/<project-name>/
  Board.md
  Inbox.md
  Jobs/YYMMDD-seq-<job-name>.md
  Jobs/Archived/YYMMDD-seq-<job-name>.md
  Tasks/YYMMDD-seq-<task-name>.md
```

Unarchived Jobs live directly under `Jobs/`; only explicitly archived Jobs move into `Jobs/Archived/`. Archiving and restoring a Job preserve its filename. Migration removes empty legacy `Jobs/Active/` and `Jobs/Archive/YYYY/MM/` directories after publishing the replacement documents and links.

`Dashboard.base` is a compact native Bases table with Name, Status, and Updated columns, sorted by recent activity. Each project name links directly to its Board. Filters include only generated, active project Boards in the configured output directory; archived projects are hidden. Formula columns display database-derived values without making project state editable in the table. The Obsidian Dashboard also has a **Pending review** Kanban view of all unarchived PENDING_REVIEW Jobs in the configured directory, ordered by `pending_review_at` ascending (oldest first). A separate `Recent Jobs.base` shows unarchived Jobs across all projects in ACTIVE, PENDING_REVIEW, COMPLETED, and CANCELLED columns, with at most ten Jobs per status ordered by `updated_at` descending. The default board uses the Taskix Sync `taskixRecentJobs` adapter around TaskNotes Kanban, preserving its card interactions while limiting entries independently for each status. Four native status tables provide the same ten-result limit without the custom view. Cards show a named project Board link and local update/review timestamps. Approval, rejection, and cancellation move Jobs between columns; archival removes them. Sync publishes the replacement before removing the registered legacy `Pending Review.base`, and protects unrelated destination files. The Projects view remains scoped to active project Boards. Sync publishes the replacement before removing the old registered Dashboard; an unrelated existing `Dashboard.base` is preserved and reported as a conflict.

Names preserve Unicode and spaces. IDs stay in YAML frontmatter, not filenames. Only collisions add `-2`, `-3`, etc.; comparison is case insensitive. `job create --name` and `task add --name` accept a concise summary separately from the full `--title`. Names default to a portable, at most 48-character title. Agents should summarize the work when choosing `--name`, rather than rely on truncation. `job update --name` and `task update --name` also work on completed work and update generated links without adding Plan versions.

Job and Task Plan filenames begin with `YYMMDD-seq-`, for example `260905-0001-Implement login.md`. The date is the Job or Task creation date in UTC, even if its Plan is created later. Sequence numbers start at 1 each day, independently for Jobs and Tasks in each project (Tasks across Jobs share the project counter), and use at least four digits. Allocation is transactional; archived or cancelled work keeps its number. Renaming and Plan revisions preserve the prefix, and display names remain concise. The `sequence` property is stored with Job and Task metadata and the Task’s Plan frontmatter.

Generated Markdown documents have YAML frontmatter containing an ID, creation time, and a type tag. `Dashboard.base` contains native Base YAML with a generated-file comment instead of Markdown frontmatter. Job properties include IDs, sequence, status, review policy, revision, and creation/update/start/completion/cancellation/archive times. They omit `document_path`, `title`, `name`, and embedded `task`/`tasks` fields; the Job heading and Task note links remain in the document body. Task properties include `revision` and lifecycle times, without `version`. All generated document lifecycle fields use `xxx_at`: `created_at`, `updated_at`, `started_at`, `completed_at`, `pending_review_at`, `followup_at`, `cancelled_at`, and `archived_at`, where applicable. Sync removes the aliases `dateCreated`, `dateModified`, `completedDate`, `created`, and `updated`, retaining canonical values when both exist. Legacy Inbox timestamps migrate without changing their values or authored contents. TaskNotes field mappings point to `created_at`, `updated_at`, and `completed_at`, so TaskNotes edits also avoid duplicate fields. Job and Task timestamps are ISO 8601 in the computer’s local time zone, with the offset for each timestamp. Job `pending_review_at` records the latest entry into review, stays unchanged through metadata edits and acceptance/rejection, and advances on resubmission. Job `followup_at` records the latest supplementary request. Project timestamps remain UTC. CLI JSON timestamps remain Unix seconds. Project `Board.md` also provides `updated_at`, derived from the latest project creation/archive or Job/Task update timestamp; syncing alone does not advance it. It records the repository root, Git remote, revision, archive state, `sync_status`, and a project-specific `sync_sequence`; unrelated projects do not advance that receipt. Board is the project note and also embeds its task view. Its ID is the Project ID. Sync migrates generated `meta.md` information into Board, updates Dashboard and task project links to Board, and deletes the old managed meta file after publishing the replacement documents. Board has no separate Project link.

Job and Task frontmatter includes managed `agent` and `session_id` properties. Jobs retain their creator's identity; Tasks initially record their creator and switch to the most recent claimant when claimed. Releasing, completing, or archiving work preserves these values. Pass `--executor agent:HOST --session HOST_SESSION` on `job create` and `task add`, as well as `task claim`; `HOST` is `codex`, `claude`, `pi`, or `omp`. Plain host names and `agent:HOST:SESSION` executor references are also recognized. Pi/OMP's structured tool supplies both flags automatically. Unknown agents and missing session IDs remain `null`; sync adds these fields to older notes using stored identity where available, without guessing a Job creator. Authored Plan properties cannot override these fields. Session IDs identify provenance and do not grant a lease.

| Document | Tags |
| --- | --- |
| Job | `agent/job` |
| Archived Job | `agent/archived/job` |
| Task (including its Plan) | `agent/task`, `task` |
| Project Board | `agent/project`, `agent/board` |
| Human Inbox | `agent/inbox` |

Each Task has one TaskNotes-compatible note in `Tasks/`, created even before a Plan is published. Notes carry `task` for TaskNotes identification and `agent/task` for Agentix board filtering. Sync adds missing tags to existing notes and preserves custom tags. Its frontmatter contains the Task ID, optional Plan ID, state, phase, revision, local dates, project link, Job link, and `dependencies`: a list of prerequisite Task IDs, or `[]`. Register all known initial Tasks and dependencies before implementation. When taking up a Task, claim it and publish its freely structured Plan into that same note. `plan revise` updates it in place and advances the Task revision. The document exposes only `revision`; the internal Plan publication counter remains part of CLI metadata. Authored properties are merged with managed metadata. LF and CRLF frontmatter delimiters are recognized, including a closing delimiter at end of file; authored body line endings are preserved. Quoted YAML keys round-trip safely. Frontmatter alone does not satisfy the nonempty Plan body required by `task start`. Dependency fields are generated from SQLite, refreshed by sync, and cannot be overridden by authored Plan properties; `task start` requires all prerequisites to be DONE.

Jobs store an optional original user `prompt` separately from the acceptance Goal. Pass it verbatim with `job create --prompt "Original user request"`; `job update JOB_ID --prompt "..."` can backfill or correct it while the Job is active, and an empty string clears it. Nonempty prompts appear in a generated **Prompt** section as literal text, preserving Markdown source and line breaks without interpreting embedded taskix markers. The prompt stays in SQLite across sync, document recreation, renaming, and archival, and is also included in IM Job details. Existing Jobs without this field default to an empty prompt and keep their previous document layout. Use the updated taskix for all writers so older versions do not drop the new field when rewriting Job records. The taskix-manager skill instructs agents to supply the original request in its original language, independently of the language used for summaries and Plans.

Job task sections contain a dependency graph followed by a Task board. Open Task notes from graph nodes or board cards; individual Task links are not listed again. Board embeds separate Job and Task Bases, with Job status columns in pastel colors. Completed and cancelled work remains visible until its Job or Project is archived. Goal, Notes, names, and Plan prose are preserved as authored.

Each nonempty Job task section also includes a generated Mermaid dependency graph. Every Task in the Job appears, including independent Tasks; arrows point from prerequisite to dependent Task. Direct prerequisites from other Jobs appear once with their Job name, without expanding those Jobs' full dependency graphs. Task additions, renames, and dependency changes refresh the graph automatically; `taskix sync` adds it to existing Job documents. The graph is read-only and uses the same database dependencies as the Task notes. Direct edges are omitted when another path in the displayed graph already connects the same Tasks. Stored dependencies and execution gates remain unchanged; paths through unexpanded external Jobs do not hide edges. Dependencies are displayed in the graph. Sync removes legacy task-link lists, block anchors, and per-task reason/dependency lines from existing Jobs while preserving authored Goal and Notes content. Task reasons remain available in their Task notes.

Graph nodes show `Task name · STATUS` using the seven `TaskStatus` values: `TODO`, `IN_PROGRESS`, `BLOCKED`, `WAITING_USER`, `DONE`, `FAILED`, and `CANCELLED`. Their light background colors match the bundled [TaskNotes status configuration](../plugins/taskix-manager/obsidian/tasknotes-settings.json), with dark text for contrast; only `DONE` means successful completion. Planning and executing are phases within `IN_PROGRESS`, not additional statuses. Status changes through taskix refresh the graph, including cross-Job prerequisite nodes.

Clicking a node's label opens its Task note. The graph uses an HTML internal link inside the node with a vault-relative file path, supporting normal navigation and hover previews without changing Mermaid security settings. Renames and Job archival regenerate the target paths. Use the Task board cards to open Task notes when graph links are unavailable. Nodes display database state at the last projection or sync; they do not embed editable TaskNotes widgets or read customized vault colors.

### Task language

Task language is a skill preference, configured with `AGENT_TASK_LANG` in the agent host environment. It controls task decomposition, Job/Task titles and concise names, goals, Notes, and Plan prose. The default is English (`en`) when unset or blank; `zh-CN` selects Chinese, and other languages such as `ja` are passed to the agent without a CLI allowlist. Explicit language instructions for the current work take precedence.

```fish
set -Ux AGENT_TASK_LANG zh-CN
```

For a macOS desktop host launched outside fish, set the environment before restarting that host:

```sh
launchctl setenv AGENT_TASK_LANG zh-CN
```

Codex/Claude hooks and Pi/OMP extensions include `task_language` in injected agent context. This field belongs to the plugin, not `taskix context`. taskix has no language option, ignores `AGENT_TASK_LANG` and the obsolete `TASKIX_LANGUAGE`, and uses fixed English labels for generated sections. Supplied names and prose remain unchanged; changing the skill preference does not translate existing documents.

When upgrading, rename the host environment variable to `AGENT_TASK_LANG`, remove `TASKIX_LANGUAGE`, and remove `[documents].language` from taskix configuration. The config loader tolerates and ignores that legacy key so existing installations still open; new configs and CLI context no longer include it. Restart desktop hosts to inherit the new environment.

### Obsidian plugin setup

After `taskix init`, run:

```sh
taskix obsidian setup
# Use a different taskix configuration:
taskix --config /path/to/taskix.toml obsidian setup --json
```

The command uses `documents.root` from the selected configuration. It installs the tested TaskNotes 4.12.5 release from its official GitHub repository when the plugin is missing, installs/enables the embedded desktop Taskix Sync plugin and enables TaskNotes and Bases in the vault configuration, and merges task identification, required field mappings, the seven Task statuses, three additional Job statuses, and the default `TODO` status. It preserves unrelated settings, custom status values, and other plugins. An existing compatible TaskNotes 4.x installation (4.12.5 or newer) is retained without a download. No task database is opened or modified.

When files change, setup automatically reloads the configured vault window through [Obsidian CLI](https://help.obsidian.md/cli). Enable **Settings → General → Command line interface** and make `obsidian` available on `PATH`. The CLI may launch Obsidian if it is not running. Setup explicitly selects the vault and verifies its canonical path before making app changes; it will not reload a different vault with the same name. Each CLI call has a ten-second timeout.

Before publishing files, setup temporarily disables already-enabled Taskix Sync and TaskNotes, then rereads configuration to preserve settings written during app startup or plugin shutdown. If installation fails, it attempts to re-enable those plugins. If reload fails after installation, it keeps the installed files and enabled list intact for a manual restart; suspended plugins may remain inactive until then. The result includes `reloaded`, `reload_error`, and `restart_required`. A successful reload command sets `reloaded: true` and `restart_required: false`; this confirms the request was accepted, not that every plugin finished loading. Otherwise `restart_required` remains true because the running app state is unconfirmed, including when unchanged files cause setup to skip CLI calls.

If the CLI is missing or fails, setup still installs the files and reports the reason with instructions to restart Obsidian. For offline setup, close Obsidian first and run `taskix obsidian setup --no-reload`, then reopen it; this flag skips all Obsidian CLI calls. If Restricted mode is enabled, turn it off in **Settings → Community plugins** to allow TaskNotes and Taskix Sync to load; setup does not change this app-local permission. Avoid editing the same configuration through Obsidian or other tools during setup.

For offline installation or an explicit plugin replacement, download `manifest.json`, `main.js`, and `styles.css` from a compatible [official TaskNotes release](https://github.com/callumalpass/tasknotes/releases) into one directory:

```sh
taskix obsidian setup --plugin-dir /path/to/tasknotes-release
```

Changed existing files are backed up under `.obsidian/taskix-backups/setup-*/`, retaining their relative paths. The result reports the backup directory, installed version, and whether anything changed. Repeating setup with unchanged settings creates no further backups. Invalid settings or plugin bundles and symlinked configuration paths are rejected before publication; write failures attempt to restore the previous files and report any rollback errors. To restore a backup, close Obsidian and copy the saved files back to the corresponding paths under `.obsidian/`.

For manual setup, use the following settings.

Enable [TaskNotes](https://tasknotes.dev/obsidian/core-concepts/) and Obsidian's Bases core plugin. Set TaskNotes' identification tag to `task`, map `dateCreated`/`dateModified`/`completedDate` to `created_at`/`updated_at`/`completed_at`, and configure the seven exact status values: `TODO`, `IN_PROGRESS`, `BLOCKED`, `WAITING_USER`, `DONE`, `FAILED`, and `CANCELLED`. Add ACTIVE, PENDING_REVIEW, and COMPLETED for the Job board; DONE and COMPLETED represent successful completion; disable automatic archival for these statuses. See the [TaskNotes setup guide](../plugins/taskix-manager/obsidian/README.md) for labels, colors, settings, and examples.

`Board.md` embeds two `tasknotesKanban` Bases: Job board first (ACTIVE, PENDING_REVIEW, COMPLETED, CANCELLED), then the seven-column Task board. Each view pins its status columns and hides unrelated empty columns. Filters select the exact Jobs/Tasks folder, project ID, the corresponding `agent/job` or `agent/task` tag, and `archived != true`. Job notes expose their name as `title` and canonical lifecycle dates through the TaskNotes field mappings without acquiring the `task` tag. Cards open their notes; status dragging is handled by Taskix Sync. Each view sorts by `updated_at` ascending, with filename as a stable tie-breaker; generated configuration does not persist manual order within a column.

Every Job note also embeds a **Task board** subsection at the end of **Tasks**, directly after the dependency graph and immediately before **Notes**. It uses the same seven status columns, sorting, and Taskix Sync status controls as the project Task board, filtered to that Job's ID, project, task folder, and tag. Empty Jobs include the view, and archived Jobs retain their task history in it. Run `taskix sync` after upgrading to add the view to existing Job notes while preserving authored Goal and Notes. The embedded board is omitted from IM Job details.

Taskix Sync settings contain the current executable's absolute `cliPath` and the selected absolute `configPath` on first setup; existing values are preserved. `--plugin-dir` only selects the TaskNotes bundle. The sync plugin is embedded in the CLI and included in the release package. Use its settings to change paths or check the connection.

Taskix Sync filters vault file events to `documents.directory` and its subdirectories, using the connected CLI configuration. Unrelated notes outside that directory do not trigger synchronization. Moves into or out of the directory still refresh registered paths. With `documents.directory = "."`, the whole vault is monitored. A failed initial connection pauses automatic event handling until a successful connection check.

The plugin reads configuration through `obsidian connection` and queries individual records through `obsidian show ID`, using IDs from note frontmatter or Inbox markers. It validates each returned path before acting on edits. Startup reconciles open notes; other notes are checked when opened or edited. At most 128 recently confirmed records are cached, alongside pending edits, instead of retaining the complete snapshot. Install the updated CLI and plugin together. Full `obsidian snapshot` remains available for diagnostics.

Normal writes use indexed entity and relationship queries, with SQL summaries for Job readiness and lightweight name/number queries for creation and renaming. They retain the same revision, lease, dependency, and lifecycle guards. Lease expiry scans only the expiry index, and lease changes update individual rows. Session heartbeats query current leases instead of loading completed Tasks.

The write transaction also records affected documents in a durable SQLite queue. A Task change updates its note, its Job summary and dependent Jobs' graphs, the relevant Project Board, and any changed Inbox entries. Job renaming or archival also refreshes its Task links and archive properties; Project archival affects that Project's notes. Obsidian Bases query vault notes dynamically; their definitions and query results are not stored in SQLite. Incremental synchronization preserves existing registered `Dashboard.base` and `Recent Jobs.base` files, including settings saved by Obsidian without generated comments, and recreates missing files. Only their managed paths are registered in SQLite. Explicit full synchronization rebuilds their definitions. Generated files retain authored regions and properties and are replaced atomically only when their contents change.

Document paths are registered individually instead of rewriting a database-wide JSON map. Pending work is deduplicated per document and read in batches of 128. Each successful publication acknowledges only the generation it read, so concurrent later edits remain queued. A failed document remains pending while other documents are attempted. The next write or `taskix sync --pending` retries that queue, including after restart; Taskix Sync uses this incremental retry after a `projection_pending` response. `doctor` reports outstanding queue entries as unhealthy even when the event cursor has not advanced.

Plain `taskix sync` remains a full repair/rebuild and imports all Project Inboxes. Schema 12 migrates the path registry and schedules one full rebuild after upgrade. Format or layout changes also require a full sync. Wide operations such as Project archival, deletion, and large dependency changes still process their affected relationship scope; ordinary writes no longer load or project every Task in the database. Persistence builds borrowed indexes for the before/after entities and a single latest-session lookup per Job, avoiding repeated vector scans during change detection and deletion checks. Event order and same-timestamp session selection remain unchanged.

After updating all writers, run `taskix sync`. Schema 7 migrates registered Plan paths to `Tasks/`; sync preserves authored content and properties and removes old managed files after publishing replacements. Board and Job paths remain stable. Task notes use the Task ID as `id`, and keep the Plan ID separately in `plan_id`. New document creation does not open tabs. taskix does not change vault-wide plugin settings. Sync removes the obsolete managed `Tasks.md` list and its navigation links; Board is the project’s only status view.

### Project archival

```sh
taskix project archive PROJECT_ID
taskix project list --archived
taskix project unarchive PROJECT_ID
```

Complete or cancel all Jobs before archiving a Project. An archived Project is hidden from the Dashboard and the default project list, keeps its documents and history, and rejects new work until restored. Job archive/unarchive remains independent; project unarchive does not unarchive individual Jobs.

### Deleting work

```sh
taskix job delete JOB_ID
taskix project delete PROJECT_ID
```

`job delete` permanently removes the Job record and document, its Tasks, dependencies, and Plan records/files, including archived work. Other Jobs and their documents remain. `project delete` removes every Job, Task, and Plan belonging to the Project, then removes its entire `Projects/<project-name>/` directory, including manually added notes, hidden files, and attachments. Its repository directory is outside this cleanup scope. Neither command requires prior archival or completion.

Release active Task leases before deleting. Job deletion rejects dependencies from Tasks in surviving Jobs; remove those dependencies explicitly first. Dependencies wholly within a deleted Project are removed together. Both commands accept `--expect-revision` and `--idempotency-key`; no interactive prompt is added. Audit events and idempotency results remain in SQLite; unfiltered `event list` includes deletion events. Deleted Job/Task filename sequence numbers are not reused within the surviving Project.

Database removal and file-cleanup records commit in one transaction. File failures return `projection_pending`; fix the reported issue and run `taskix sync`, or retry the exact delete request with the same idempotency key. Cleanup survives process restarts. A Project name whose directory is still pending deletion cannot be registered until cleanup completes. Cleanup refuses paths redirected through symlinks, and removes nested attachment symlinks without following their targets.

### Read-only boundary

`Inbox.md` imports saved human requirements and cancellations. Taskix Sync also listens to saved Obsidian Job/Task status edits, including TaskNotes dragging, and registered Inbox checkbox edits, sending supported transitions to taskix. Failed or unsupported changes restore authoritative status, completion dates or the affected checkbox and show a Notice. See [supported edits and recovery](../plugins/taskix-manager/obsidian/README.md#status-edits). The plugin never claims work or borrows an agent lease.

Other managed fields remain read-only projections. Existing Base definitions are preserved during incremental synchronization; explicit full synchronization regenerates them. Goal/Notes markers preserve their editable bodies, and custom Job/Task properties survive synchronization. Explicit `job update --goal` replaces a manually edited Goal; Notes remain untouched. Missing/duplicated editable markers fail synchronization instead of dropping content. With the plugin disabled, status edits are local drift and the next projection restores them. Startup reconciliation never replays offline edits as commands. There is no CLI `watch` daemon.

Database commits happen before generated-document updates. A filesystem failure returns success with `projection_pending` so callers do not recreate committed work. `taskix sync` repairs the projection. Output file locks serialize independent CLI processes; temporary-file replacement protects each document. Archival writes the destination and updates managed links before removing the previous generated file. A Plan replacement body is committed with its lease check and retained until projection acknowledges it, so interrupted publication can be retried. Back up the database and document tree together; editable bodies are not recoverable from SQLite alone.

```sh
taskix doctor --json  # healthy, missing_plans, database/projection sequence
taskix sync
```

### Data coverage and recovery

TaskNotes renders the views; the bundled Taskix Sync desktop plugin submits status edits. Enable Obsidian's built-in Bases core plugin as well. TaskNotes indexes Markdown files, so the saved notes and board remain readable if taskix's SQLite database is lost. Agent task execution and synchronization still depend on SQLite.

The vault is not a complete database export:

| Data | Representation in Obsidian |
| --- | --- |
| Current project and Job identity, hierarchy, lifecycle, and revision | Metadata, Job properties, filenames, links, and prose; not a serialized copy of every database field |
| Current Task identity, status, phase, revision, sequence, and lifecycle dates | Task frontmatter |
| Plan body, custom properties, Goal, and Notes | Authored note content; SQLite does not retain a complete copy after publication |
| Task dependencies | Prerequisite Task IDs in Task frontmatter `dependencies`, with navigable links in Job prose; no import contract |
| Task reasons | Displayed in Job prose; not exported in Task frontmatter |
| Agent and session provenance | Job creator and Task creator/latest claimant in managed `agent` and `session_id` frontmatter |
| Task ordering and execution bookkeeping | Position, full executor reference, delegation, and system-block flag are not fully exported |
| Ownership leases | Tokens and expiration times are not exported |
| Audit history and request idempotency | Event log, request fingerprints, and saved results remain in SQLite |
| Internal Plan publication state | Publication counters, hashes, and pending unpublished bodies are not fully exported; document `revision` does not replace the internal publication counter |
| Synchronization and cleanup state | Managed-path bookkeeping, pending deletions, and sequence counters that prevent reuse of deleted filenames remain in SQLite; `Board.md` exposes sync status and sequence |

There is no general vault import or database rebuild command; Inbox import only handles the human queue. `sync` projects the database into documents; it does not reconstruct database records from existing frontmatter. Notes could support a future partial reconstruction of current work after validation, but cannot reproduce missing history, ownership, or retry records. Restored work would need fresh claims rather than recovered lease tokens.

For a restorable backup:

1. Pause agent/CLI writers and note edits, run `taskix sync`, and confirm `taskix doctor --json` reports a healthy projection.
2. Create a database snapshot using the [SQLite backup API](https://sqlite.org/backup.html) or the SQLite shell's `.backup` command. Do not rely on copying only the main file of a live WAL database.
3. Back up the matching document tree and taskix configuration while writes remain paused. Include TaskNotes settings to preserve the Obsidian display configuration.
4. Restore the matched database, documents, and configuration with writers stopped. Validate the restored copy separately with `doctor`, then `sync` and `doctor` before resuming work. Recover/reclaim active work through the normal lease workflow.

If only the vault survives, preserve a copy before further taskix writes. Existing task notes can still be browsed with TaskNotes, but normal agent execution requires a database backup or a separately implemented, explicitly partial reconstruction.

## Host plugin

The shared package is `plugins/taskix-manager`, included in the standalone `taskix-*` release archives, not the `agentix-*` archives. Use a taskix archive or a source checkout for the plugin; The Homebrew formula installs the CLI, embedded Taskix Sync resources, completions, and example configuration; the formula is maintained exclusively in [`tenfyzhong/homebrew-tap`](https://github.com/tenfyzhong/homebrew-tap/blob/main/Formula/taskix.rb). Install host plugins separately using the commands below. It has Codex/Claude manifests, a shared Skill, command hooks, and Pi/OMP TypeScript entrypoints. Node.js 22 or newer is required for command hooks. Put `taskix` on PATH or set `TASKIX_BIN`; set `TASKIX_CONFIG` when using a non-default config.

Install through the repository's `agentix` marketplace in Codex and Claude Code. Add the repository/worktree root as the marketplace, then install `taskix-manager@agentix`. Codex uses `codex plugin marketplace add` followed by `codex plugin add`; Claude Code uses `claude plugin marketplace add` followed by `claude plugin install`. The catalogs are `.agents/plugins/marketplace.json` and `.claude-plugin/marketplace.json`. See the [complete installation commands](../plugins/taskix-manager/README.md#prerequisites-and-activation), including when GitHub-based installation is available.

Claude merges default discovery of the shared hook file with manifest-selected `hooks/claude.json` for explicitly interrupted tool failures. Codex explicitly loads that file plus `hooks/codex.json` for Interrupt; its manifest replaces default discovery, avoiding duplicate hooks. Review/enable hooks in the host as required; Codex requires reviewing and trusting plugin hooks through `/hooks`. The command resolves plugin-root environment variables inside Node rather than using shell-specific expansion.

The package explicitly includes its manifests, hooks, extensions, runtime, skills, and activation guide in npm distributions. Pi and OMP each select their own entrypoint through `package.json`; installing the complete package does not require copying hooks into project settings. See the [plugin activation and lifecycle guide](../plugins/taskix-manager/README.md).

For Pi/OMP, install dependencies and use the host's `install` command on the complete plugin directory from a source checkout or taskix release archive:

```sh
npm ci --ignore-scripts --prefix /absolute/path/to/taskix-manager
pi install /absolute/path/to/taskix-manager
omp install /absolute/path/to/taskix-manager
```

Run only the install command for your chosen host, then restart or reload it. Both hosts load the selected extension and the shared Skill from `package.json`; keep the local package at a stable path. Obsidian editing requires the user's separate Obsidian skill package; taskix's generated structure is deterministic and does not launch an Agent itself.

SessionStart restores eligible Tasks and supplies task context. SessionEnd blocks active work and releases leases. Codex Interrupt does the same with reason `session interrupted` for an interrupted active main-thread turn; it preserves the Plan, fences the old token, and allows deletion once all relevant leases are released. Subsequent heartbeats do not reacquire released leases. Stop renews leases without claiming Inbox work or requesting a continuation. Inbox intake requires explicit user input for each next Job. Tool hooks renew at tool boundaries; no hook daemon is spawned. A Codex/Claude operation or idle gap longer than 15 minutes can expire a lease. Pi/OMP extensions renew every minute while active; detected interruption and shutdown stop the timer and cancel in-flight renewal before releasing leases. Pi waits for agent_settled after an aborted result; OMP uses agent_end and excludes willContinue. New work restarts heartbeat without implicitly reclaiming a Task. The extensions inject current task facts before the agent runs, and expose a structured taskix tool with session, executor, current lease token, and request idempotency key. Full Task IDs and unambiguous Task prefixes receive the same lease injection; prefixes are resolved before matching the owned Task, and ambiguous prefixes are rejected. Retried writes keep their original injected credentials. A stale-token rejection requires inspecting and reacquiring work, never forcing a completion.

The shared SessionEnd hook, Codex Interrupt hook, and Claude PostToolUseFailure hook each request three seconds. Claude also has an overall session-exit budget, defaulting to 1.5 seconds; plugin hook timeouts do not raise it. Set `CLAUDE_CODE_SESSIONEND_HOOKS_TIMEOUT_MS=3000` when additional shutdown time is needed. If a busy database or interrupted process prevents shutdown cleanup, the next task operation reaps the expired lease. Other command hooks allow 30 seconds. Claude only releases on PostToolUseFailure when is_interrupt is the boolean true; ordinary tool errors do nothing. Claude has no general interrupt hook, and cancellation may emit no failure event. Use explicit cleanup after stopping work in that case. Pi/OMP aborts without an aborted assistant result also require shutdown or explicit cleanup. Disconnecting an idle Codex CLI from a persistent app-server does not guarantee Interrupt or immediate SessionEnd. After stopping the agent, `taskix hook session-end --session SESSION_ID` explicitly releases its active leases; `taskix hook interrupt --session SESSION_ID` records interruption instead. Update both taskix and the installed plugin, then review the changed Codex hooks through `/hooks`; see the [lifecycle guide](../plugins/taskix-manager/README.md#lifecycle-behavior).

Pi/OMP supplies an idempotency key for metadata mutations, including Job/Project deletion. Retrying the same deletion tool call returns the committed result without duplicate events. Within one Pi/OMP extension instance, the most recent 512 write requests retain their original injected lease token for idempotent retries, including after a successful write releases the lease or its response is lost. This token cache is not persisted across host restarts. Retrying beyond that window must preserve the original CLI request explicitly; do not assume a newly discovered lease will replay the old request.

Host session references remain unchanged so Agentix bindings can route notifications. Team context belongs to future Team tooling, keyed by `job_id`; `context --json`, cursor-based events, and optional `--delegated-by team:<id>` provide the integration boundary.

## Agentix integration

After initializing taskix, add to Agentix's configuration:

```toml
[task_board]
enable = true
config = "~/.config/taskix/config.toml"
```

`task_board.enable` defaults to `false`; a `config` path alone does not enable the integration. When disabled, Agentix does not load the taskix configuration or start the task board, and IM menus and help omit task-board commands. When `enable = true`, the referenced configuration must exist; its SQLite database is created if missing. Select the same taskix configuration as your task writers to browse their existing work. Restart Agentix after adding or changing `[task_board]`; taskix being configured on its own does not enable the IM integration. With `task_board.enable = true`, Telegram registers `/dashboard` in its default command menu at startup, before any attachment. The top-level menu order is `/sessions`, `/dashboard`, `/cancel`, `/rmux`, `/help` (omit `/dashboard` when disabled). Contextual commands follow in alphabetical order. `/board`, `/jobs`, `/inboxes`, and `/inbox` appear in the chat menu only after attach; `/tasks` is not added to the menu.

`/dashboard` is the top-level IM dashboard. Each unarchived project has a button that opens its task board, grouped by status with full counts and clickable task entries. Project boards can be browsed without attaching a session.

The IM dashboard aggregates Job and Task counts in SQL without deserializing their bodies. Project and session boards use SQL status counts, ordering, and pagination, returning only page-sized task summaries. Session Job lists return page-sized Job summaries and SQL task counts without authored Job or Task bodies. Job details page task summaries alongside their authored Markdown; Task details retain the selected records, parents, and leases. These reads omit Plan and Inbox bodies. Page selection precedes summary construction, and counts share the same read transaction as the page. Session associations include stored expired leases, but only unexpired leases mark tasks as current.

After attach, `/board` and `/jobs` appear as contextual secondary menu commands. `/board` shows the current session's task board; `/jobs` lists all associated unarchived Jobs. Both find Jobs containing tasks with a matching lease or last recorded session. Sibling tasks show overall Job progress, and blocked/completed work remains visible after lease release until reassignment changes the last session or archival removes the Job from lists. Only unexpired leases are marked `Current`. Switching or detaching a session changes the scope and invalidates previous navigation buttons. Sessions without associated work get an empty-state message. These commands work independently of the agent's session-control capability, including read-only attachments.

Click a Job to read its original Prompt and authored Goal and Notes as Markdown, with buttons for its associated tasks and project board. Click a task from a board or Job to read its Task note body and current metadata. Every Task detail page includes a **Job** button for returning to its parent Job. YAML frontmatter, generated local task links and dependency graphs are excluded from the IM detail body. Telegram uses the existing MarkdownV2 conversion; Feishu uses its Markdown card element. Unavailable documents are reported while metadata and navigation remain accessible.

Project/Job/task lists have six entries per page with **Previous**/**Next** controls. Long detail bodies and Task reasons are also paged, preserving fenced code blocks across pages. Detail headers and button labels use at most 60 characters; full Task/Job titles longer than this are included in the paginated detail body. For Job details, pagination advances both the authored content and associated task buttons; further pages remain available until both are exhausted. Browsing is read-only and does not update Plan hashes. Callback tokens use the existing conversation/owner, generation and binding-epoch checks.

`/projects` and `/sessionboard` are replaced by `/dashboard` and `/board`; `/board` and `/jobs` always use the current attachment. Legacy `/tasks [job-or-project]` and `/task <id>` remain direct shortcuts; the legacy task list is capped at 50 entries. An attached session can claim an unplanned Task or operate its own lease. Start is offered in PLANNING when Plan metadata and dependencies are ready; the service verifies the file at execution time. Done is offered only in EXECUTING. Block/Wait/Fail request a reason; `/cancel` clears pending input. Buttons use existing owner/conversation, generation, and binding-epoch checks plus the Task revision. IM can append human requirements to the Project Inbox; agents create the formal Job and Tasks on intake. IM does not edit Plan bodies.

Agentix incrementally consumes SQLite events during its existing runtime tick. WAITING_USER, BLOCKED, FAILED, and Job pending-review, rejection, and completion notifications go only to the matching bound session's conversation. Events without a matching binding are skipped. Agentix atomically writes notifications and its ingestion cursor to its runtime database before sending. Existing taskix consumer cursors are imported once. Up to 32 independent workers deliver the oldest pending notice per conversation, each with a 20-second deadline. Failed sends retry after exponential delays of 1–256 seconds; interrupted deliveries become available when their 60-second leases expire. A slow conversation does not hold the ingestion cursor or another conversation’s delivery. Delivery is at least once: a crash after IM accepts a send but before its local acknowledgment can duplicate the notice. Acknowledged notices are removed, and replaying their event IDs cannot recreate them. CLI-only usage does not require Agentix to run.

## Validation

`make check` installs locked plugin dependencies, then runs Rust formatting, Clippy, workspace tests, and the Node built-in plugin tests. Install Node.js 24+ and npm. Direct Cargo invocations require `npm ci --ignore-scripts --prefix plugins/taskix-manager` first. Normal tests use temporary databases/directories and local mock services, not live accounts.

| Boundary | Automated coverage |
| --- | --- |
| State and ownership | Seven Task states with IN_PROGRESS split into two phases against ten commands (80 cases), no partial writes on rejection, claim-before-Plan, Plan/start ownership, missing/blank Plans, lease renewal/recovery/handoff, stale tokens, dependency changes, archival |
| Processes and storage | Eight competing CLI processes with exactly one claim winner; four concurrent Jobs; start waits for Plan writes and rechecks lease expiry; v1 phase migration preserves leases/timestamps; kill after SQLite commit but before projection, then replay without duplicate events and repair files |
| Document projection | Legacy Dashboard migration to Bases, collision preservation and retry, archive visibility and activity sorting; seven-state Mermaid graphs with cross-Job prerequisites and renamed links; TaskNotes Bases and per-task Obsidian notes, exact folder/project scope, frontmatter state projection and repair without SQLite writes, legacy Plan-path migration, editable Notes under concurrent writes, safe marker/symlink failures, and YAML-frontmatter Plan bodies |
| Host plugin | Actual Pi/OMP TypeScript entrypoints and lifecycle hooks invoke the compiled CLI; structured tool schema, plans, leases, Obsidian wikilinks, retry identity after lease release/lost responses and deletion, ordinary Claude failures and automatic Pi/OMP continuations that retain ownership, errors, aborts, identity fencing, and periodic heartbeat behavior |
| IM orchestration | Dashboard → project board → Task ↔ Job navigation, attached-session scope and released work, sorted contextual menus, project/Job/task pagination, archive filtering, missing-document fallbacks, long Markdown/reasons/titles, read-only snapshots; session/revision/owner scoping, Wait/Fail reasons, cancellation, Job completion, notification paging, route isolation, retry after channel failure, durable delivery cursor after Engine reconstruction |
| Channel adapters | Actual Telegram HTTP and Feishu HTTP/WebSocket adapters exercise dashboard and detail navigation before attachment, attached `/board` and `/jobs`, and MarkdownV2/card output; task callbacks and reason messages verify SQLite state, projected Markdown, and notifications at local mock APIs |

The plugin tests use a minimal host API harness, not installed Pi/OMP loaders or model-generated tool calls. Codex/Claude hook tests execute their manifest commands with representative payloads, not live host sessions. CI runs the normal suite on Linux/macOS and task core/CLI/plugin checks on Windows; Unix-only symlink cases are excluded on Windows.

For actual Obsidian rendering, enable the ignored desktop tests explicitly. Open a test vault with TaskNotes and Bases enabled, bring its window to the foreground, enable the Obsidian CLI, and ensure the chosen parent directory already exists:

```sh
TASKIX_OBSIDIAN_VAULT="Test vault" TASKIX_OBSIDIAN_PARENT="Tests" \
  cargo test -p taskix --test obsidian_smoke -- --ignored --nocapture
```

`OBSIDIAN_BIN` can select a specific CLI executable. For each format, the test creates an isolated `taskix-smoke-*` directory under that parent (default `00-Inbox/agent`) and a temporary tab, then restores the previous tab and deletes only its own generated files. It checks Dashboard columns, dates, archive/unarchive filtering and link targets through native navigation, plus separate Job/Task Kanban columns and cards, task note recognition, note links, and the Mermaid state diagrams. The status bridge scenario loads the bundled plugin under a temporary ID against an isolated database, edits real frontmatter, and verifies rollback notifications and successful CLI writes. The visibility prerequisite prevents hidden-window rendering from being mistaken for an empty board. It temporarily extends TaskNotes status definitions in memory and loads a temporary Taskix Sync instance through Obsidian's plugin loader; cleanup unloads it and restores the previous status definitions. It does not change persistent community-plugin settings. A force-killed test process can leave its temporary directory/tab behind; do not run it concurrently with manual edits in that directory.

The [integration coverage map](integration-coverage.md) links each behavior to its executable tests. These tests do not establish complete branch coverage or live-system acceptance. Real IM credentials/permissions, host installer and loader compatibility, model-directed tool selection, desktop themes/plugins, and multi-machine/network-filesystem behavior require separate checks. The supported concurrency target remains multiple local processes on one computer.

## Job conversation records

Job documents place **Prompt** and then **Conversation** at the end, immediately after **Notes**. Synchronizing existing documents applies this order while preserving authored Goal and Notes content. Job detail views use the same order, omitting generated task navigation and graphs.

Job documents preserve the actual original user request under **Prompt** and follow-up user inputs under **Conversation → Turn N → User input**. Conversation is divided into ordered **Turn N** sections, each with **User input** and **Agent output**. Visible assistant messages within each turn are combined, in order, into one Markdown blockquote. Supplementary prompts append turns while preserving the original Prompt and earlier agent outputs. User input is rendered literally; assistant Markdown is rendered inside the quote, and quoted control-marker text cannot modify managed document sections. Conversation data lives in SQLite and survives synchronization, renaming, and document recovery.

Agentix collects completed user/assistant message items and records them when the turn ends, even without an IM attachment, so a new prompt is not assigned before its Job is created. Codex and Claude Stop hooks read the current turn from the host transcript when `transcript_path` is supplied; Pi/OMP record visible messages at `agent_end`. By default, tool calls, tool results, reasoning, and system/developer messages are excluded. Agentix can include host-exposed reasoning and tool details with `[output] show_reasoning = true` and/or `show_tool_calls = true` in its configuration. These independent switches apply to IM turn output and the associated Job’s Agent output. IM process output uses separate Reasoning and Output sections when process details are present. Reasoning start labels are omitted; only actual host-provided reasoning content is shown. Tool events are deduplicated by item ID; recording happens at turn completion. Standalone Taskix host hooks continue to capture visible user/assistant messages only. Known leading host-context wrappers, including injected `# AGENTS.md instructions` with `<INSTRUCTIONS>` and `<environment_context>`, are also removed from user-role messages at capture and persistence. Actual requests mentioning AGENTS.md remain intact. Sync applies the same filtering when rendering old stored conversations and prompts. Existing Job prompts are preserved; the first recorded user message fills a missing prompt. Automatic capture applies to new turns after upgrading the host plugin; it does not reconstruct unavailable historical transcripts.

A host can submit a JSON array of `{ "id": "stable-message-id", "role": "user" | "assistant", "text": "visible text" }` through `taskix hook record --session SESSION --file messages.json`, optionally with `--job JOB_ID`. Only Jobs associated with that session are eligible; otherwise the most recently worked associated Job is selected. No eligible Job is a no-op. Replaying the same message ID does not duplicate it. A late final response can be recorded after Task completion without changing the Job's review state. The operation does not claim work or acquire a Task lease.

### Supplementary requests and review policy

Context and host hooks expose `previous_job` as a candidate for a new prompt. The agent decides whether the request supplements that PENDING_REVIEW Job. If so, `taskix job followup JOB_ID --prompt "Verbatim supplementary request" --executor agent:HOST --session HOST_SESSION` returns the same Job to ACTIVE and appends the request without replacing its original Prompt. Supply the actual current host identity and session on follow-up to associate subsequent conversation capture with that session. Existing Tasks remain; each Task added for this supplement automatically depends on the snapshot of all old Tasks taken at follow-up. Additional dependencies among new Tasks are configured normally. Cancelled and other unfinished prerequisites do not satisfy the execution gate. Independent requirements and requests after a COMPLETED Job create new Jobs; hooks do not reopen Jobs merely because a prompt arrived.

Git and gh delivery requests such as `git commit`, `git push`, `gh pr create`, and `gh pr edit` also supplement a pending Job when they concern its changes, even if the prompt only says "commit" or "create a PR". Resolve that ownership before creating a Job or choosing a review policy. Use the conversation and repository/worktree evidence to confirm the delivery; if `context.previous_job` is absent, inspect the current Project with `job list --pending-review` rather than assuming the request is independent. Tool names alone do not establish relevance. For a match, run `job followup` before delivery work, then add new Tasks to the same ACTIVE Job with the old Tasks as dependencies. Preserve the whole Job's review policy: a Git-only supplement to an implementation Job still requires review. Only independent operational Jobs use `none`; unrelated requests and requests after COMPLETED get new Jobs.

`job create` and `job update` accept `--review-policy required|none`. Creation defaults to `required`; updates that omit the flag preserve the existing policy. Investigation-only, document-only, and simple operational Jobs such as git commit/push use `none`: when at least one non-cancelled Task exists and all are DONE, the Job goes directly to COMPLETED, and its Inbox entry completes with it. No separate human approval is needed. Code-change Jobs and mixed Jobs retain `required` and enter PENDING_REVIEW when ready. If supplementary work adds code changes, update the policy to `required` for the whole Job.
